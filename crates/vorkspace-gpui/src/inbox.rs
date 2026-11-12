//! Window-local connector panes and their retained detail state.
mod avatar;
mod catalog;
mod diff;
mod document;
mod feed;
mod saved_preferences;
mod ui;

use crate::{
    git_tools::diff_view::{DiffEvent, DiffView},
    input::{self, Input},
    popover::AnchorSlot,
};
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment,
    ListState, Pixels, ScrollHandle, Subscription, WeakEntity, px,
};
use std::{path::PathBuf, rc::Rc};
use vorkspace_core::inbox::{
    self as model, Checks, Details, Diff, Item, Service, Snapshot, Thread, ViewState,
};
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::theme::*;

pub(crate) use catalog::SessionLinks;
pub(crate) use feed::{FeedEvent, InboxFeed};

pub(crate) enum Event {
    Close,
    ConnectorAvailabilityChanged,
    OpenTab(Box<Item>),
    OpenSession(i64),
    StartSession {
        item: Box<Item>,
        prompt: String,
        bot: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Summary,
    Code,
    Checks,
}

struct PostedComment {
    key: String,
    revision: u64,
    reply: Option<(String, String)>,
}

pub(crate) struct Inbox {
    /// The root retains the pane entities. Each pane owns its details and inputs.
    owner: Option<WeakEntity<Inbox>>,
    feed: Entity<InboxFeed>,
    pages: std::collections::HashMap<String, Entity<Inbox>>,
    store: Database,
    service: Service,
    projects: Vec<PathBuf>,
    state: ViewState,
    preferences: Entity<saved_preferences::SavedPreferences>,
    saved_state: ViewState,
    snapshot: Rc<Snapshot>,
    sessions: Rc<catalog::RelatedSessions>,
    publication_queued: bool,
    focus: FocusHandle,
    search: Entity<Input>,
    comment: Entity<Input>,
    comment_revision: u64,
    summary_scroll: ListState,
    list_scroll: ScrollHandle,
    summary_width: std::rc::Rc<std::cell::Cell<Pixels>>,
    activity: Vec<ui::Activity>,
    activity_revision: u64,
    summary_rows: std::cell::RefCell<ui::SummaryRows>,
    focus_comment: bool,
    selected: Option<String>,
    /// A detail-only page keeps its item identity when a list refresh omits it.
    /// The last available summary is retained until a matching item arrives.
    pinned_item: Option<Item>,
    /// Animated leaf width from the containing workspace, when present.
    pane_width: Option<f32>,
    release_pending: bool,
    narrow_detail: bool,
    details: Option<Details>,
    thread: Option<Thread>,
    document: Option<Entity<document::InboxDocument>>,
    comments: Vec<(model::Comment, Entity<document::InboxDocument>)>,
    diff: Option<Diff>,
    diff_view: Entity<DiffView>,
    checks: Option<Checks>,
    tab: Tab,
    loading: bool,
    detail_loading: bool,
    extra_loading: bool,
    posting: bool,
    acting: bool,
    error: Option<String>,
    detail_target_error: Option<String>,
    detail_error: Option<String>,
    extra_error: Option<String>,
    post_error: Option<String>,
    thread_error: Option<String>,
    expanded_comments: std::collections::HashSet<String>,
    merge_action: String,
    reply: Option<(String, String)>,
    detail_generation: u64,
    extra_request: u64,
    filters_open: bool,
    filter_anchor: AnchorSlot,
    connect_provider: Option<String>,
    connect_url: Entity<Input>,
    connect_email: Entity<Input>,
    connect_token: Entity<Input>,
    connecting: bool,
    connection_revision: u64,
    connect_error: Option<String>,
    start_project: PathBuf,
    bots: Vec<(String, String)>,
    start_bot: Option<String>,
    destination_explicit: bool,
    project_picker: bool,
    project_anchor: AnchorSlot,
    action_menu: bool,
    action_anchor: AnchorSlot,
    pr_action_anchors: [AnchorSlot; 4],
    action_error: Option<String>,
    action_notice: Option<String>,
    confirm_action: Option<String>,
    resize_start: Option<(f32, f32)>,
    active: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Event> for Inbox {}
impl Focusable for Inbox {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Inbox {
    pub(crate) fn new(
        store: Database,
        projects: Vec<PathBuf>,
        cwd: PathBuf,
        poll_projects: Vec<PathBuf>,
        shared: (&Entity<SessionLinks>, &Entity<InboxFeed>),
        cx: &mut Context<Self>,
    ) -> Self {
        let (links, feed) = shared;
        let mut inbox = Self::new_inner(store, projects, cwd, feed.clone(), cx);
        let root = cx.entity_id();
        feed.update(cx, |feed, cx| feed.set_scope(root, poll_projects, cx));
        cx.observe(feed, Self::adopt_feed).detach();
        cx.subscribe(feed, |this, _, event: &FeedEvent, cx| {
            if matches!(event, FeedEvent::DetailTick) {
                this.refresh_active_details(cx);
            }
        })
        .detach();
        inbox.sessions = links.read(cx).index();
        cx.observe(links, |this, links, cx| {
            this.sessions = links.read(cx).index();
            this.publish_pages(cx);
            cx.notify();
        })
        .detach();
        inbox
    }
    fn new_inner(
        store: Database,
        mut projects: Vec<PathBuf>,
        cwd: PathBuf,
        feed: Entity<InboxFeed>,
        cx: &mut Context<Self>,
    ) -> Self {
        if !cwd.as_os_str().is_empty() && !projects.contains(&cwd) {
            projects.push(cwd.clone());
        }
        let preferences = saved_preferences::SavedPreferences::get(&store, cx);
        let state = Self::project_preferences(&preferences.read(cx).state, None);
        let data_dir = locked(&store)
            .ok()
            .and_then(|db| db.data_dir().map(PathBuf::from))
            .unwrap_or_else(|| cwd.clone());
        cx.observe(&preferences, |this, owner, cx| {
            let saved = owner.read(cx).state.clone();
            this.adopt_preferences(&saved, true, cx);
        })
        .detach();
        let entity = cx.entity_id();
        let feed_on_release = feed.downgrade();
        cx.on_release(move |_, cx| {
            let _ = feed_on_release.update(cx, |feed, cx| feed.release(entity, cx));
        })
        .detach();
        let (snapshot, loading, connection_revision) = {
            let shared = feed.read(cx);
            (
                shared.snapshot.clone(),
                shared.loading(),
                shared.connection_revision,
            )
        };
        let placeholder = crate::controls::page::secondary_ink();
        let search = cx.new(|cx| Input::field("Filter items", "Filter items", placeholder, cx));
        let comment = cx.new(Input::inbox_comment);
        let connect_url = cx.new(|cx| Input::field("Instance URL", "https://", placeholder, cx));
        let connect_email = cx.new(|cx| Input::field("Email", "you@example.com", placeholder, cx));
        let connect_token = cx.new(|cx| {
            let mut input = Input::field("API token", "Personal access token", placeholder, cx);
            input.set_secret();
            input
        });
        let diff_view = cx.new(DiffView::new);
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, _: &input::Changed, cx| cx.notify()),
            cx.subscribe(&comment, |this, _, _: &input::Changed, cx| {
                this.comment_revision = this.comment_revision.wrapping_add(1);
                let count = this.summary_scroll.item_count();
                this.summary_scroll
                    .remeasure_items(count.saturating_sub(1)..count);
                cx.notify();
            }),
            cx.subscribe(&comment, |this, _, _: &input::Submitted, cx| {
                this.post_comment(cx)
            }),
            cx.subscribe(&diff_view, |this, view, event: &DiffEvent, cx| {
                if let DiffEvent::Comment(request) = event
                    && view.read(cx).owns_comment(request)
                {
                    this.comment
                        .update(cx, |input, cx| input.set_text(request.text.clone(), cx));
                    view.update(cx, |view, cx| view.complete_comment(request, Ok(()), cx));
                    this.tab = Tab::Summary;
                    this.summary_scroll.scroll_to_end();
                    this.focus_comment = true;
                    cx.notify();
                }
            }),
        ];
        Self {
            owner: None,
            feed,
            pages: Default::default(),
            service: Service {
                cwd: if cwd.is_dir() {
                    cwd.clone()
                } else {
                    data_dir.clone()
                },
                data_dir,
            },
            store,
            projects,
            saved_state: state.clone(),
            state,
            preferences,
            snapshot,
            sessions: Default::default(),
            publication_queued: false,
            focus: cx.focus_handle(),
            search,
            comment,
            comment_revision: 0,
            summary_scroll: ListState::new(0, ListAlignment::Top, px(200.)),
            list_scroll: ScrollHandle::new(),
            summary_width: Default::default(),
            activity: vec![],
            activity_revision: 0,
            summary_rows: Default::default(),
            focus_comment: false,
            selected: None,
            pinned_item: None,
            pane_width: None,
            release_pending: false,
            narrow_detail: false,
            details: None,
            thread: None,
            document: None,
            comments: vec![],
            diff: None,
            diff_view,
            checks: None,
            tab: Tab::Summary,
            loading,
            detail_loading: false,
            extra_loading: false,
            posting: false,
            acting: false,
            error: None,
            detail_target_error: None,
            detail_error: None,
            extra_error: None,
            post_error: None,
            thread_error: None,
            expanded_comments: Default::default(),
            merge_action: "merge".into(),
            reply: None,
            detail_generation: 0,
            extra_request: 0,
            filters_open: false,
            filter_anchor: AnchorSlot::default(),
            connect_provider: None,
            connect_url,
            connect_email,
            connect_token,
            connecting: false,
            connection_revision,
            connect_error: None,
            start_project: cwd,
            bots: vec![],
            start_bot: None,
            destination_explicit: false,
            project_picker: false,
            project_anchor: AnchorSlot::default(),
            action_menu: false,
            action_anchor: AnchorSlot::default(),
            pr_action_anchors: std::array::from_fn(|_| AnchorSlot::default()),
            action_error: None,
            action_notice: None,
            confirm_action: None,
            resize_start: None,
            active: false,
            _subscriptions: subscriptions,
        }
    }
    pub(crate) fn set_pane_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = width.is_finite().then_some(width.max(0.));
        if self.pane_width != width {
            self.pane_width = width;
            if width.is_some_and(|width| width < 900.) {
                self.resize_start = None;
            }
            cx.notify();
        }
    }

    pub(crate) fn set_context(
        &mut self,
        projects: Vec<PathBuf>,
        cwd: PathBuf,
        poll_projects: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.projects != projects || self.start_project != cwd {
            self.projects = projects;
            self.service.cwd = if cwd.is_dir() {
                cwd.clone()
            } else {
                self.service.data_dir.clone()
            };
            self.start_project = cwd;
            self.publish_pages(cx);
            cx.notify();
        }
        let root = cx.entity_id();
        self.feed
            .update(cx, |feed, cx| feed.set_scope(root, poll_projects, cx));
    }
    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        let entity = cx.entity_id();
        self.feed
            .update(cx, |feed, _| feed.set_active(entity, active));
        if active {
            if let Some(item) = self.item() {
                self.state.seen.mark(&item);
                self.save(cx);
                if self.pinned_item.is_some()
                    && (self.details.is_none() || self.detail_target_error.is_some())
                    && !self.detail_loading
                {
                    self.load_detail(self.details.is_none(), cx);
                }
            } else if self.pinned_item.is_none()
                && self.selected.is_none()
                && let Some(key) = self.first_visible_key(cx)
            {
                self.select(key, cx);
            }
        }
        cx.notify();
    }
    #[cfg(test)]
    pub(crate) fn unread_count(&self) -> usize {
        self.snapshot
            .items
            .iter()
            .filter(|item| {
                self.state.connectors.enabled(&item.provider) && self.state.seen.unseen(item)
            })
            .count()
    }

    pub(crate) fn connector_preferences(&self) -> model::Connectors {
        self.state.connectors.clone()
    }
    pub(crate) fn jira_filter(&self) -> model::JiraFilter {
        self.state.jira_filter.clone()
    }
    pub(crate) fn connection_revision(&self) -> u64 {
        self.connection_revision
    }
    pub(crate) fn default_bots(&self) -> model::BotDefaults {
        self.state.default_bots.clone()
    }
    pub(crate) fn jira_projects(&self) -> Vec<(String, String)> {
        self.snapshot
            .jira_projects
            .iter()
            .map(|row| (row.id.clone(), format!("{} · {}", row.key, row.name)))
            .collect()
    }
    pub(crate) fn hidden_jira_projects(&self) -> Vec<String> {
        self.state.hidden_jira_projects.clone()
    }
    pub(crate) fn set_jira_filter(
        &mut self,
        filter: model::JiraFilter,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let saved = self.preferences.update(cx, |saved, cx| {
            saved.update_store(|store| filter.save(store), cx)
        })?;
        self.adopt_preferences(&saved, true, cx);
        self.publish_pages(cx);
        cx.notify();
        Ok(())
    }
    pub(crate) fn set_jira_project(&mut self, id: &str, visible: bool, cx: &mut Context<Self>) {
        self.state.hidden_jira_projects.retain(|value| value != id);
        if !visible {
            self.state.hidden_jira_projects.push(id.into());
        }
        self.filters_changed(cx);
    }
    pub(crate) fn set_default_bot(
        &mut self,
        source: &str,
        bot: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let saved = self.preferences.update(cx, |saved, cx| {
            saved.update_store(
                |store| {
                    let mut defaults = model::BotDefaults::load(store);
                    defaults.set(source, bot)?;
                    defaults.save(store)
                },
                cx,
            )
        })?;
        self.adopt_preferences(&saved, true, cx);
        self.publish_pages(cx);
        if !self.destination_explicit {
            self.reset_destination();
        }
        cx.notify();
        Ok(())
    }
    pub(crate) fn set_bots(&mut self, bots: Vec<(String, String)>, cx: &mut Context<Self>) {
        if self.bots != bots {
            self.bots = bots;
            if self.owner.is_none() {
                self.publish_pages(cx);
            }
            if !self.destination_explicit {
                self.reset_destination();
            }
            if self
                .start_bot
                .as_ref()
                .is_some_and(|id| !self.bots.iter().any(|(key, _)| key == id))
            {
                self.start_bot = None;
            }
            cx.notify();
        }
    }
    pub(crate) fn set_connector_enabled(
        &mut self,
        provider: &str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let saved = self.preferences.update(cx, |saved, cx| {
            saved.update_store(
                |store| {
                    let mut connectors = model::Connectors::load(store);
                    let before = connectors.clone();
                    connectors.set(provider, enabled)?;
                    if connectors != before {
                        connectors.save(store)?;
                    }
                    Ok(())
                },
                cx,
            )
        })?;
        self.adopt_preferences(&saved, true, cx);
        self.feed
            .update(cx, |feed, cx| feed.preferences_changed(cx));
        self.adopt_feed(self.feed.clone(), cx);
        if self.item().is_none() {
            self.selected = None;
            self.narrow_detail = false;
            self.details = None;
            self.thread = None;
            self.document = None;
            self.comments.clear();
            self.diff = None;
            self.checks = None;
            self.detail_generation += 1;
            self.extra_request += 1;
            self.detail_loading = false;
            self.extra_loading = false;
        }
        if self
            .connect_provider
            .as_deref()
            .is_some_and(|key| !self.state.connectors.enabled(key))
        {
            self.connect_provider = None;
            self.connect_token
                .update(cx, |input, cx| input.set_text(String::new(), cx));
        }
        self.save(cx);
        self.publish_pages(cx);
        cx.notify();
        Ok(())
    }
    pub(crate) fn connection_statuses(&self) -> std::collections::HashMap<String, (bool, String)> {
        self.state
            .connectors
            .enabled_providers()
            .map(|provider| {
                let status = self.snapshot.connections.get(provider);
                let connected = status.is_some_and(|status| status.connected);
                let description =
                    self.snapshot
                        .errors
                        .get(provider)
                        .cloned()
                        .unwrap_or_else(|| match status {
                            Some(status) if !status.connected => "Not connected".into(),
                            Some(status) if !status.site.is_empty() => status.site.clone(),
                            Some(status) if !status.url.is_empty() => status.url.clone(),
                            Some(_) => "Connected".into(),
                            None => "Not connected".into(),
                        });
                (provider.into(), (connected, description))
            })
            .collect()
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.feed.update(cx, |feed, cx| feed.refresh(true, cx));
    }

    fn adopt_feed(&mut self, feed: Entity<InboxFeed>, cx: &mut Context<Self>) {
        let shared = feed.read(cx);
        let loading = shared.loading();
        let connection_revision = shared.connection_revision;
        let snapshot_changed = !Rc::ptr_eq(&self.snapshot, &shared.snapshot);
        let loading_changed = self.loading != loading;
        if !snapshot_changed && !loading_changed && self.connection_revision == connection_revision
        {
            return;
        }
        self.snapshot = shared.snapshot.clone();
        self.loading = loading;
        self.connection_revision = connection_revision;
        if snapshot_changed {
            if self
                .item()
                .is_none_or(|item| item.provider != self.state.source)
            {
                self.selected = None;
                self.narrow_detail = false;
                self.details = None;
                self.document = None;
                self.comments.clear();
            }
            if self.active
                && let Some(item) = self.item()
            {
                self.state.seen.mark(&item);
                self.save(cx);
            } else if self.active
                && let Some(key) = self.first_visible_key(cx)
            {
                self.select(key, cx);
            }
        }
        if snapshot_changed || loading_changed {
            self.publish_pages(cx);
        }
        cx.notify();
    }

    fn refresh_active_details(&self, cx: &mut Context<Self>) {
        for page in self.pages.values() {
            if !page.read(cx).active {
                continue;
            }
            page.update(cx, |page, cx| {
                if page.selected.is_none() || page.detail_loading || page.posting || page.acting {
                    return;
                }
                page.load_detail(false, cx);
                if page.tab == Tab::Checks && !page.extra_loading {
                    page.checks = None;
                    page.set_tab(Tab::Checks, cx);
                }
            });
        }
    }

    #[cfg(test)]
    fn new_for_test(
        store: Database,
        projects: Vec<PathBuf>,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) -> Self {
        let poll_projects = if projects.is_empty() {
            vec![cwd.clone()]
        } else {
            projects.clone()
        };
        let data_dir = locked(&store)
            .ok()
            .and_then(|db| db.data_dir().map(PathBuf::from))
            .unwrap_or_else(|| cwd.clone());
        let feed = cx.new(|cx| InboxFeed::new(&store, data_dir, cx));
        let links = cx.new(|_| SessionLinks::default());
        Self::new(store, projects, cwd, poll_projects, (&links, &feed), cx)
    }

    #[cfg(test)]
    fn stage_fixture_poll(&mut self, cx: &mut Context<Self>) -> u64 {
        let generation = self.feed.update(cx, |feed, cx| {
            feed.snapshot = self.snapshot.clone();
            feed.stage_fixture_poll(cx)
        });
        self.adopt_feed(self.feed.clone(), cx);
        generation
    }

    #[cfg(test)]
    fn finish_fixture_poll(&mut self, generation: u64, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.feed.update(cx, |feed, cx| {
            feed.finish_fixture_poll(generation, snapshot, cx)
        });
        let saved = self.preferences.read(cx).state.clone();
        self.adopt_preferences(&saved, true, cx);
        self.adopt_feed(self.feed.clone(), cx);
    }

    fn project_preferences(saved: &ViewState, source: Option<&str>) -> ViewState {
        let mut state = saved.clone();
        if let Some(source) = source {
            state.source = source.into();
        }
        for provider in model::PROVIDERS {
            state
                .provider_filters
                .entry(provider.into())
                .or_insert_with(|| saved.filters.clone());
        }
        state.filters = saved.filters_for(&state.source).clone();
        state
    }

    fn adopt_preferences(
        &mut self,
        saved: &ViewState,
        preserve_edits: bool,
        cx: &mut Context<Self>,
    ) {
        let source = if self.owner.is_some() || saved.connectors.enabled(&self.state.source) {
            Some(self.state.source.as_str())
        } else {
            None
        };
        let mut next = Self::project_preferences(saved, source);
        let baseline = next.clone();
        if preserve_edits {
            next.seen.seeded |= self.state.seen.seeded;
            for (key, revision) in &self.state.seen.items {
                let seen = next.seen.items.entry(key.clone()).or_default();
                *seen = (*seen).max(*revision);
            }
            if self.state.list_width != self.saved_state.list_width {
                next.list_width = self.state.list_width;
            }
            if self.state.filters != self.saved_state.filters {
                next.filters = self.state.filters.clone();
            }
            for (provider, filter) in &self.state.provider_filters {
                if self.saved_state.provider_filters.get(provider) != Some(filter) {
                    next.provider_filters
                        .insert(provider.clone(), filter.clone());
                }
            }
        }
        self.saved_state = baseline;
        if next != self.state {
            let defaults_changed = self.state.default_bots != next.default_bots;
            let connectors_changed = self.state.connectors != next.connectors;
            self.state = next;
            if defaults_changed && !self.destination_explicit {
                self.reset_destination();
            }
            if connectors_changed && self.owner.is_none() {
                cx.emit(Event::ConnectorAvailabilityChanged);
            }
            cx.notify();
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saved_state == self.state {
            return;
        }
        let result = self.preferences.update(cx, |saved, cx| {
            saved.save_edits(
                &self.saved_state,
                &self.state,
                self.pinned_item.is_none(),
                self.owner.is_none(),
                cx,
            )
        });
        match result {
            Ok(saved) => self.adopt_preferences(&saved, false, cx),
            Err(error) => self.error = Some(error),
        }
    }
    #[cfg(test)]
    pub(crate) fn suspend_refresh_for_test(&mut self) {
        self.loading = true;
    }
    pub(crate) fn saved_source(&self) -> &str {
        &self.state.source
    }
    pub(crate) fn remember_source(&mut self, source: &str, cx: &mut Context<Self>) {
        self.state.source = source.into();
        self.state.filters = self.state.filters_for(source).clone();
        self.save(cx);
        self.feed.update(cx, |feed, cx| feed.refresh_if_stale(cx));
    }
    pub(crate) fn provider_unread_count(&self, provider: &str) -> usize {
        self.snapshot
            .items
            .iter()
            .filter(|item| {
                item.provider == provider
                    && self.item_in_scope(item)
                    && self.state.connectors.enabled(provider)
                    && self.state.seen.unseen(item)
            })
            .count()
    }
    pub(crate) fn connector_page(&mut self, source: &str, cx: &mut Context<Self>) -> Entity<Self> {
        if let Some(page) = self.pages.get(source) {
            return page.clone();
        }
        let owner = cx.weak_entity();
        let page = cx.new(|cx| {
            let mut page = Self::new_inner(
                self.store.clone(),
                self.projects.clone(),
                self.start_project.clone(),
                self.feed.clone(),
                cx,
            );
            page.owner = Some(owner);
            page.state = self.state.clone();
            page.state.source = source.into();
            page.state.filters = self.state.filters_for(source).clone();
            page.saved_state = page.state.clone();
            page.snapshot = self.snapshot.clone();
            page.sessions = self.sessions.clone();
            page.bots = self.bots.clone();
            page.loading = self.loading;
            page
        });
        self.pages.insert(source.into(), page.clone());
        page
    }
    pub(crate) fn connector_item_page(
        &mut self,
        source: &str,
        mut item: Item,
        cx: &mut Context<Self>,
    ) -> Entity<Self> {
        assert_eq!(source, item.provider, "item page provider must match item");
        let item_key = item.detail_key();
        let page_key = format!("item:{item_key}");
        if let Some(page) = self.pages.get(&page_key) {
            page.update(cx, |page, _| page.release_pending = false);
            return page.clone();
        }
        if let Some(current) = self
            .snapshot
            .items
            .iter()
            .find(|current| current.detail_key() == item_key)
        {
            Self::refresh_pinned_summary(&mut item, current);
        }
        let owner = cx.weak_entity();
        let page = cx.new(|cx| {
            let mut page = Self::new_inner(
                self.store.clone(),
                self.projects.clone(),
                if item.project_path.is_empty() {
                    self.start_project.clone()
                } else {
                    PathBuf::from(&item.project_path)
                },
                self.feed.clone(),
                cx,
            );
            page.owner = Some(owner);
            page.state = self.state.clone();
            page.state.source = source.into();
            page.state.filters = self.state.filters_for(source).clone();
            page.saved_state = page.state.clone();
            page.snapshot = self.snapshot.clone();
            page.sessions = self.sessions.clone();
            page.bots = self.bots.clone();
            page.loading = self.loading;
            page.selected = Some(item_key);
            page.pinned_item = Some(item);
            page
        });
        self.pages.insert(page_key, page.clone());
        page
    }
    pub(crate) fn release_item_page(&mut self, item: &Item, cx: &mut Context<Self>) {
        let page_key = format!("item:{}", item.detail_key());
        let Some(page) = self.pages.get(&page_key) else {
            return;
        };
        let busy = page.update(cx, |page, cx| {
            page.set_active(false, cx);
            page.release_pending = true;
            page.posting || page.acting
        });
        if !busy {
            self.pages.remove(&page_key);
        }
    }
    fn finish_pending_release(&mut self, cx: &mut Context<Self>) {
        if !self.release_pending || self.posting || self.acting {
            return;
        }
        let (Some(owner), Some(item)) = (self.owner.clone(), self.pinned_item.as_ref()) else {
            return;
        };
        let page_key = format!("item:{}", item.detail_key());
        let page_entity = cx.entity();
        let _ = owner.update(cx, |owner, _| {
            if owner.pages.get(&page_key) == Some(&page_entity) {
                owner.pages.remove(&page_key);
            }
        });
    }
    pub(crate) fn pinned_item(&self) -> Option<&Item> {
        self.pinned_item.as_ref()
    }

    fn refresh_pinned_summary(pinned: &mut Item, current: &Item) -> bool {
        if current.detail_key() != pinned.detail_key() || current.updated_ms() < pinned.updated_ms()
        {
            return false;
        }
        // Provider snapshots can prefer another clone of this repository.
        // Keep this tab's checkout for relative links and its conversation target.
        let project = std::mem::take(&mut pinned.project_path);
        *pinned = current.clone();
        pinned.project_path = project;
        true
    }

    fn item_in_project(item: &Item, project: &str) -> bool {
        (item.provider == "jira" && item.project_path.is_empty())
            || item.belongs_to_project(project)
    }

    fn scoped_project<'a>(&self, item: &'a Item) -> Option<&'a str> {
        item.project_paths().find(|path| {
            let key = vorkspace_core::files::path_key(path);
            self.projects
                .iter()
                .any(|p| vorkspace_core::files::path_key(&p.to_string_lossy()) == key)
        })
    }

    fn item_in_scope(&self, item: &Item) -> bool {
        item.project_path.is_empty() || self.scoped_project(item).is_some()
    }

    fn scoped_item(&self, item: &Item) -> Item {
        let mut scoped = item.clone();
        if let Some(path) = self.scoped_project(item) {
            scoped.project_path = path.into();
        }
        scoped
    }

    pub(crate) fn item_by_key_for_project(&self, key: &str, project: &str) -> Option<Item> {
        self.snapshot
            .items
            .iter()
            .find(|item| item.key() == key && Self::item_in_project(item, project))
            .cloned()
            .map(|mut item| {
                item.project_path = project.to_owned();
                item
            })
    }

    pub(crate) fn items_for_project(&self, project: &str, limit: usize) -> (Vec<Item>, bool) {
        let mut items = self
            .snapshot
            .items
            .iter()
            .filter(|item| Self::item_in_project(item, project));
        let selected = items.by_ref().take(limit).cloned().collect();
        (selected, items.next().is_some())
    }
    pub(crate) fn pane_title(&self) -> String {
        if self.pinned_item.is_none() {
            return model::provider_label(&self.state.source).into();
        }
        let item = self.item().expect("pinned page retains its item");
        if item.title.is_empty() {
            item.reference()
        } else {
            item.title
        }
    }
    /// Publish the complete root snapshot before delivering it to pages. Reading
    /// the latest owner at delivery avoids replaying an older queued snapshot.
    fn publish_pages(&mut self, cx: &mut Context<Self>) {
        if self.pages.is_empty() || self.publication_queued {
            return;
        }
        self.publication_queued = true;
        let owner = cx.weak_entity();
        cx.defer(move |cx| {
            let Ok((state, snapshot, sessions, projects, service, cwd, bots, loading, pages)) =
                owner.update(cx, |root, _| {
                    root.publication_queued = false;
                    (
                        root.state.clone(),
                        root.snapshot.clone(),
                        root.sessions.clone(),
                        root.projects.clone(),
                        root.service.clone(),
                        root.start_project.clone(),
                        root.bots.clone(),
                        root.loading,
                        root.pages.values().cloned().collect::<Vec<_>>(),
                    )
                })
            else {
                return;
            };
            for page in pages {
                page.update(cx, |page, cx| {
                    let scope_changed = page.projects != projects;
                    page.projects = projects.clone();
                    page.service = service.clone();
                    if scope_changed
                        && page.pinned_item.is_none()
                        && !page.projects.contains(&page.start_project)
                    {
                        page.start_project = cwd.clone();
                    }
                    page.set_bots(bots.clone(), cx);
                    page.loading = loading;
                    // Disabled/temporarily unavailable providers retain selection,
                    // details and drafts. Their page identity never changes.
                    if state.connectors.enabled(&page.state.source) {
                        page.snapshot = snapshot.clone();
                        page.sessions = sessions.clone();
                        if let Some(pinned) = page.pinned_item.as_mut() {
                            let key = pinned.detail_key();
                            if let Some(current) =
                                snapshot.items.iter().find(|item| item.detail_key() == key)
                            {
                                Self::refresh_pinned_summary(pinned, current);
                            }
                        }
                        if page.pinned_item.is_none()
                            && page.active
                            && page.selected.is_none()
                            && let Some(key) = page.first_visible_key(cx)
                        {
                            page.select(key, cx);
                        } else if page.active
                            && let Some(item) = page.item()
                        {
                            page.state.seen.mark(&item);
                            page.save(cx);
                        }
                    }
                    cx.notify();
                });
            }
        });
    }
    fn visible_items<'a>(&'a self, cx: &App) -> impl Iterator<Item = &'a Item> + use<'a> {
        let filter = self.state.filters.prepare(&self.search.read(cx).content);
        self.snapshot.items.iter().filter(move |item| {
            item.provider == self.state.source
                && self.state.connectors.enabled(&item.provider)
                && self.item_in_scope(item)
                && filter.matches(item, &self.state.source)
        })
    }
    fn first_visible_key(&self, cx: &App) -> Option<String> {
        self.visible_items(cx).next().map(Item::detail_key)
    }
    fn item(&self) -> Option<Item> {
        if let Some(pinned) = &self.pinned_item {
            return Some(pinned.clone());
        }
        self.item_in_list(self.selected.as_deref()?)
    }
    fn item_in_list(&self, key: &str) -> Option<Item> {
        self.snapshot
            .items
            .iter()
            .find(|item| {
                item.provider == self.state.source
                    && item.detail_key() == key
                    && self.item_in_scope(item)
            })
            .map(|item| self.scoped_item(item))
    }
    fn select(&mut self, key: String, cx: &mut Context<Self>) {
        if self
            .pinned_item
            .as_ref()
            .is_some_and(|item| item.detail_key() != key)
        {
            return;
        }
        if self.selected.as_ref() == Some(&key) {
            return;
        }
        self.selected = Some(key);
        self.destination_explicit = false;
        self.reset_destination();
        self.summary_scroll = ListState::new(0, ListAlignment::Top, px(200.));
        self.activity.clear();
        self.activity_revision += 1;
        self.summary_rows = Default::default();
        self.focus_comment = false;
        self.tab = Tab::Summary;
        self.diff = None;
        self.extra_request += 1;
        self.diff_view
            .update(cx, |view, cx| view.set_files(vec![], cx));
        self.checks = None;
        self.extra_loading = false;
        self.extra_error = None;
        self.details = None;
        self.thread = None;
        self.detail_target_error = None;
        self.document = None;
        self.comments.clear();
        self.reply = None;
        self.post_error = None;
        self.thread_error = None;
        self.expanded_comments.clear();
        self.merge_action = "merge".into();
        self.confirm_action = None;
        self.action_error = None;
        self.action_notice = None;
        self.action_menu = false;
        self.comment
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        if let Some(item) = self.item() {
            if self.active {
                self.state.seen.mark(&item);
                self.save(cx);
            }
            if !item.project_path.is_empty() {
                self.start_project = PathBuf::from(&item.project_path);
            }
        }
        self.load_detail(true, cx);
        cx.notify();
    }
    fn reset_destination(&mut self) {
        self.start_bot = self
            .item()
            .and_then(|item| self.state.default_bots.get(&item.provider))
            .filter(|id| self.bots.iter().any(|(key, _)| key == id))
            .map(str::to_owned);
    }
    fn change_source(&mut self, source: String, cx: &mut Context<Self>) {
        if self.owner.is_some() || !self.state.connectors.enabled(&source) {
            return;
        }
        self.state.filters = self.state.filters_for(&source).clone();
        self.state.source = source;
        self.selected = None;
        self.narrow_detail = false;
        self.details = None;
        self.document = None;
        self.comments.clear();
        self.search
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        if let Some(key) = self.first_visible_key(cx) {
            self.select(key, cx);
        }
        self.save(cx);
        cx.notify();
    }
    fn load_detail(&mut self, clear: bool, cx: &mut Context<Self>) {
        let Some(item) = self.item() else {
            return;
        };
        self.detail_generation += 1;
        let generation = self.detail_generation;
        self.detail_loading = true;
        self.detail_error = None;
        self.thread_error = None;
        if clear {
            self.document = None;
            self.comments.clear();
        }
        let key = item.detail_key();
        let service = self.service.clone();
        let task = cx
            .background_executor()
            .spawn(async move { service.discussion(&item) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.detail_generation != generation || this.selected.as_ref() != Some(&key) {
                    return;
                }
                match result {
                    Ok(discussion) => {
                        let restored = this.detail_target_error.take().is_some();
                        this.finish_detail(
                            generation,
                            key,
                            discussion.details,
                            discussion.thread,
                            cx,
                        );
                        if restored && this.tab != Tab::Summary {
                            this.set_tab(this.tab, cx);
                        }
                    }
                    Err(error) => {
                        this.detail_loading = false;
                        if this.post_error.as_ref() == Some(&error) {
                            this.post_error = None;
                        }
                        this.detail_target_error = Some(error);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    fn finish_detail(
        &mut self,
        generation: u64,
        key: String,
        details: Result<Details, String>,
        thread: Result<Thread, String>,
        cx: &mut Context<Self>,
    ) {
        if self.detail_generation != generation || self.selected.as_ref() != Some(&key) {
            return;
        }
        self.detail_loading = false;
        match details {
            Ok(details) => {
                if self
                    .details
                    .as_ref()
                    .is_none_or(|old| old.body != details.body)
                {
                    self.document = Some(cx.new(|cx| {
                        document::InboxDocument::new(details.body.clone(), self.document_cwd(), cx)
                    }));
                }
                self.details = Some(details);
            }
            Err(error) => self.detail_error = Some(error),
        }
        match thread {
            Ok(thread) => {
                let mut rows = vec![];
                for comment in &thread.comments {
                    let document = self
                        .comments
                        .iter()
                        .find(|(old, _)| old.id == comment.id && old.body == comment.body)
                        .map(|(_, document)| document.clone())
                        .unwrap_or_else(|| {
                            cx.new(|cx| {
                                document::InboxDocument::new(
                                    comment.body.clone(),
                                    self.document_cwd(),
                                    cx,
                                )
                            })
                        });
                    rows.push((comment.clone(), document));
                    for reply in &comment.replies {
                        let document = self
                            .comments
                            .iter()
                            .find(|(old, _)| old.id == reply.id && old.body == reply.body)
                            .map(|(_, document)| document.clone())
                            .unwrap_or_else(|| {
                                cx.new(|cx| {
                                    document::InboxDocument::new(
                                        reply.body.clone(),
                                        self.document_cwd(),
                                        cx,
                                    )
                                })
                            });
                        rows.push((reply.clone(), document));
                    }
                }
                self.comments = rows;
                self.thread = Some(thread);
                self.rebuild_activity();
            }
            Err(error) => self.thread_error = Some(error),
        }
        self.summary_scroll.remeasure();
        cx.notify();
    }
    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.extra_error = None;
        let Some(item) = self.item() else {
            return;
        };
        if self.detail_target_error.is_some()
            || tab == Tab::Summary
            || (tab == Tab::Code && self.diff.is_some())
            || (tab == Tab::Checks && self.checks.is_some())
        {
            self.extra_loading = false;
            cx.notify();
            return;
        }
        self.extra_loading = true;
        self.extra_request += 1;
        let request = self.extra_request;
        let key = item.detail_key();
        let service = self.service.clone();
        let task = cx.background_executor().spawn(async move {
            if tab == Tab::Code {
                service.diff(&item).map(|diff| {
                    let files = diff::models(&diff);
                    (Some((diff, files)), None)
                })
            } else {
                service.checks(&item).map(|checks| (None, Some(checks)))
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.selected.as_ref() != Some(&key)
                    || this.tab != tab
                    || request != this.extra_request
                {
                    return;
                }
                this.extra_loading = false;
                match result {
                    Ok((diff, checks)) => {
                        if let Some((diff, files)) = diff {
                            this.diff_view.update(cx, |view, cx| {
                                view.file_count = Some(diff.files.len());
                                view.totals = Some((
                                    diff.additions.max(0).min(u32::MAX as i64) as u32,
                                    diff.deletions.max(0).min(u32::MAX as i64) as u32,
                                ));
                                view.set_files(files, cx);
                            });
                            this.diff = Some(diff);
                        }
                        if let Some(checks) = checks {
                            this.checks = Some(checks);
                        }
                    }
                    Err(error) => this.extra_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn record_self_activity(&self, key: String, revision: i64, cx: &mut Context<Self>) {
        self.feed
            .update(cx, |feed, _| feed.note_mutation(key, revision));
    }

    fn post_comment(&mut self, cx: &mut Context<Self>) {
        if self.posting || self.detail_target_error.is_some() {
            return;
        }
        let Some(item) = self.item() else {
            return;
        };
        let body = self.comment.read(cx).content.trim().to_string();
        if body.is_empty() {
            return;
        }
        self.posting = true;
        self.post_error = None;
        let posted = PostedComment {
            key: item.detail_key(),
            revision: self.comment_revision,
            reply: self.reply.clone(),
        };
        let service = self.service.clone();
        let reply_id = posted
            .reply
            .as_ref()
            .map(|r| r.0.clone())
            .unwrap_or_default();
        let task = cx
            .background_executor()
            .spawn(async move { service.comment(&item, body, reply_id) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_comment(posted, result, cx);
            });
        })
        .detach();
        cx.notify();
    }
    fn finish_comment(
        &mut self,
        posted: PostedComment,
        result: Result<model::CommentReceipt, String>,
        cx: &mut Context<Self>,
    ) {
        self.posting = false;
        let selected = self.selected.as_ref() == Some(&posted.key);
        match result {
            Ok(receipt) => {
                let revision = receipt.revision.as_deref().map_or(0, model::timestamp_ms);
                self.record_self_activity(posted.key, revision, cx);
                if selected {
                    if self.comment_revision == posted.revision && self.reply == posted.reply {
                        self.comment
                            .update(cx, |input, cx| input.set_text(String::new(), cx));
                        self.reply = None;
                    }
                    if !self.release_pending {
                        self.load_detail(false, cx);
                    }
                }
                self.refresh(cx);
            }
            Err(error) if selected && self.detail_target_error.as_ref() != Some(&error) => {
                self.post_error = Some(error)
            }
            Err(_) => {}
        }
        cx.notify();
        self.finish_pending_release(cx);
    }
    fn document_cwd(&self) -> PathBuf {
        self.item()
            .filter(|item| !item.project_path.is_empty())
            .map(|item| PathBuf::from(item.project_path))
            .unwrap_or_else(|| self.start_project.clone())
    }
    fn start(&mut self, cx: &mut Context<Self>) {
        if let Some(mut item) = self.item() {
            if self.start_bot.is_none() {
                item.project_path = self.start_project.to_string_lossy().into_owned();
            }
            let body = self.details.as_ref().map(|d| d.body.as_str()).unwrap_or("");
            let discussion = self
                .comments
                .iter()
                .map(|(c, _)| format!("{}: {}", c.author, c.body))
                .collect::<Vec<_>>()
                .join("\n\n");
            let prompt = item.prompt(&format!("{body}\n\n{discussion}"));
            cx.emit(Event::StartSession {
                item: Box::new(item),
                prompt,
                bot: self.start_bot.clone(),
            });
        }
    }
    pub(crate) fn open_connection(&mut self, provider: String, cx: &mut Context<Self>) {
        if self.connecting || !self.state.connectors.enabled(&provider) {
            return;
        }
        self.connect_error = None;
        let status = self
            .snapshot
            .connections
            .get(&provider)
            .cloned()
            .unwrap_or_default();
        let url = if !status.site.is_empty() {
            status.site
        } else if !status.url.is_empty() {
            status.url
        } else if provider == "gitlab" {
            "https://gitlab.com".into()
        } else {
            String::new()
        };
        self.connect_url
            .update(cx, |input, cx| input.set_text(url, cx));
        self.connect_email
            .update(cx, |input, cx| input.set_text(status.email, cx));
        self.connect_token
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.connect_provider = Some(provider);
        cx.notify();
    }
    fn dismiss_connection(&mut self, cx: &mut Context<Self>) {
        if !self.connecting && self.connect_provider.take().is_some() {
            self.connect_token
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            cx.notify();
        }
    }
    fn connect(&mut self, disconnect: bool, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let Some(provider) = self.connect_provider.clone() else {
            return;
        };
        let url = self.connect_url.read(cx).content.to_string();
        let email = self.connect_email.read(cx).content.to_string();
        let token = if disconnect {
            String::new()
        } else {
            self.connect_token.read(cx).content.to_string()
        };
        if provider != "github" && !disconnect && token.trim().is_empty() {
            self.connect_error = Some("Enter an API token".into());
            cx.notify();
            return;
        }
        if let Err(error) = self
            .feed
            .update(cx, |feed, cx| feed.begin_connection(&provider, cx))
        {
            self.connect_error = Some(error);
            cx.notify();
            return;
        }
        self.connecting = true;
        self.connect_error = None;
        let service = self.service.clone();
        let source = provider.clone();
        let feed = self.feed.clone();
        let task = cx
            .background_executor()
            .spawn(async move { service.connect(&source, url, email, token) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            feed.update(cx, |feed, cx| {
                feed.finish_connection(&provider, result.as_ref().ok().cloned(), cx);
            });
            let _ = this.update(cx, |this, cx| {
                this.connecting = false;
                if !this.state.connectors.enabled(&provider) {
                    this.connect_token.update(cx, |input, cx| input.set_text(String::new(), cx));
                    cx.notify();
                    return;
                }
                match result {
                    Ok(status) => {
                        this.connect_token.update(cx, |input, cx| input.set_text(String::new(), cx));
                        if status.connected || disconnect {
                            this.connect_provider = None;
                            if status.connected {
                                this.change_source(provider, cx);
                            }
                        } else {
                            this.connect_error = Some("GitHub is not connected. Run “gh auth login” in a terminal, then check again.".into());
                        }
                    }
                    Err(error) => this.connect_error = Some(error),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
    fn run_action(&mut self, cx: &mut Context<Self>) {
        if self.acting {
            return;
        }
        let (Some(item), Some(action)) = (self.item(), self.confirm_action.clone()) else {
            return;
        };
        self.acting = true;
        self.action_error = None;
        let service = self.service.clone();
        let key = item.detail_key();
        let merging = matches!(action.as_str(), "merge" | "squash" | "rebase");
        let task = cx
            .background_executor()
            .spawn(async move { service.action(&item, &action) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.acting = false;
                match result {
                    Ok(item) => {
                        this.record_self_activity(key.clone(), item.updated_ms(), cx);
                        if let Some(pinned) = this.pinned_item.as_mut() {
                            Self::refresh_pinned_summary(pinned, &item);
                        }
                        let merged = item.state.eq_ignore_ascii_case("merged");
                        this.feed.update(cx, |feed, cx| feed.update_item(item, cx));
                        if this.selected.as_ref() == Some(&key) {
                            this.action_notice = (merging && !merged)
                                .then(|| "Merge queued or auto-merge enabled.".into());
                            this.confirm_action = None;
                            if !this.release_pending {
                                this.load_detail(false, cx);
                            }
                        }
                        this.refresh(cx);
                    }
                    Err(error) => this.action_error = Some(error),
                }
                cx.notify();
                this.finish_pending_release(cx);
            });
        })
        .detach();
        cx.notify();
    }
}
