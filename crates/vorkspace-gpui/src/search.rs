//! Native search using the shared compact page layout.
use crate::{
    controls::page,
    input::{self, Input},
};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle,
    MouseButton, MouseMoveEvent, Pixels, Point, Render, Role, ScrollHandle, StyledText,
    Subscription, Task, Window, div, point, prelude::*, px, relative,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use vorkspace_core::{
    search::{self, Hit, Scope, Target},
    store::Session,
    transcript::Row,
};
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::theme::*;

pub(crate) enum Event {
    Open(Target, String),
    Close,
}
#[derive(Default)]
pub(crate) struct Sources {
    pub summaries: Vec<Session>,
    pub live: HashMap<i64, Arc<Vec<Row>>>,
    pub projects: Vec<String>,
    pub recent: Vec<String>,
}
pub(crate) struct SearchView {
    pub(crate) input: Entity<Input>,
    cwd: String,
    store: Database,
    summaries: Arc<Vec<Session>>,
    live: HashMap<i64, Arc<Vec<Row>>>,
    projects: Arc<Vec<String>>,
    recent: Arc<Vec<String>>,
    files: Arc<Vec<vorkspace_core::files::ProjectFile>>,
    visible: bool,
    listing_pending: bool,
    scope: Scope,
    query: String,
    hits: Vec<Hit>,
    catalog_hits: Vec<Hit>,
    catalog_task: Option<Task<()>>,
    catalog_revision: u64,
    remote: Vec<Hit>,
    local: Vec<Hit>,
    live_task: Option<Task<()>>,
    live_revision: u64,
    content: Vec<Hit>,
    active: usize,
    scroll: ScrollHandle,
    pointer: Option<Point<Pixels>>,
    sessions_loading: bool,
    files_loading: bool,
    truncated: (bool, bool),
    error: Option<String>,
    cancel: Arc<AtomicBool>,
    listing_cancel: Arc<AtomicBool>,
    debounce: Option<Task<()>>,
    _changed: Subscription,
}
impl EventEmitter<Event> for SearchView {}
impl Focusable for SearchView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}
impl Drop for SearchView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.listing_cancel.store(true, Ordering::Release);
    }
}
impl SearchView {
    pub(crate) fn new(
        cwd: String,
        store: Database,
        sources: Sources,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| Input::field("Search", "Search everything…", page::secondary_ink(), cx));
        let changed = cx.subscribe(&input, |this, _, _: &input::Changed, cx| {
            this.query = this.input.read(cx).content.trim().to_owned();
            this.active = 0;
            this.refresh(cx);
        });
        let mut this = Self {
            input,
            cwd,
            store,
            summaries: Arc::new(sources.summaries),
            live: sources.live,
            projects: Arc::new(sources.projects),
            recent: Arc::new(sources.recent),
            files: Arc::default(),
            visible: true,
            listing_pending: false,
            scope: Scope::All,
            query: String::new(),
            hits: vec![],
            catalog_hits: vec![],
            catalog_task: None,
            catalog_revision: 0,
            remote: vec![],
            local: vec![],
            live_task: None,
            live_revision: 0,
            content: vec![],
            active: 0,
            scroll: ScrollHandle::new(),
            pointer: None,
            sessions_loading: false,
            files_loading: false,
            truncated: (false, false),
            error: None,
            cancel: Arc::new(AtomicBool::new(false)),
            listing_cancel: Arc::new(AtomicBool::new(false)),
            debounce: None,
            _changed: changed,
        };
        this.reload_files(cx);
        this.request_catalog(cx);
        this
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    /// A revealed pane receives one current snapshot from the Shell.
    pub(crate) fn set_active(&mut self, sources: Option<Sources>, cx: &mut Context<Self>) {
        self.visible = sources.is_some();
        if let Some(sources) = sources {
            self.summaries = Arc::new(sources.summaries);
            self.live = sources.live;
            self.projects = Arc::new(sources.projects);
            self.recent = Arc::new(sources.recent);
            self.live_revision += 1;
            self.reload_files(cx);
            self.refresh(cx);
        } else {
            self.listing_cancel.store(true, Ordering::Release);
            self.cancel_query();
        }
    }

    fn reload_files(&mut self, cx: &mut Context<Self>) {
        self.listing_cancel.store(true, Ordering::Release);
        self.listing_cancel = Arc::new(AtomicBool::new(false));
        // A view searches one root for its whole life, so the previous listing
        // stays usable while the refreshed one loads; clearing it would empty
        // recents and file results for the length of a large repository scan.
        self.listing_pending = true;
        let flag = self.listing_cancel.clone();
        let check = flag.clone();
        let root = self.cwd.clone();
        let work = cx.background_executor().spawn(async move {
            let mut files =
                vorkspace_core::files::list_project_files_sync_cancellable(&root, Some(&flag))?;
            // git ls-files includes cached paths deleted from the working copy.
            files.retain(|file| !flag.load(Ordering::Acquire) && Path::new(&file.path).is_file());
            Ok::<_, String>(files)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if check.load(Ordering::Acquire) {
                    return;
                }
                this.listing_pending = false;
                if let Ok(files) = result {
                    this.files = Arc::new(files);
                    this.request_catalog(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn draft(&self, cx: &App) -> (String, Scope) {
        (self.input.read(cx).content.to_string(), self.scope)
    }
    #[cfg(test)]
    pub(crate) fn hits(&self) -> &[Hit] {
        &self.hits
    }
    pub(crate) fn restore_draft(&mut self, draft: (String, Scope), cx: &mut Context<Self>) {
        self.scope = draft.1;
        self.input
            .update(cx, |input, cx| input.set_text(draft.0, cx));
    }
    pub(crate) fn select_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        self.scope = scope;
        self.active = 0;
        self.refresh(cx);
    }
    pub(crate) fn update_session(
        &mut self,
        summary: Session,
        rows: Option<Arc<Vec<Row>>>,
        cx: &mut Context<Self>,
    ) {
        let id = summary.id;
        let changed = match self.summaries.iter().position(|s| s.id == id) {
            Some(index) if self.summaries[index] != summary => {
                Arc::make_mut(&mut self.summaries)[index] = summary;
                true
            }
            Some(_) => false,
            None => {
                Arc::make_mut(&mut self.summaries).push(summary);
                true
            }
        };
        if let Some(rows) = rows {
            self.live.insert(id, rows);
        } else {
            self.live.remove(&id);
            self.local.retain(
                |hit| !matches!(hit.target, Target::Message { session, .. } if session == id),
            );
        }
        self.live_revision += 1;
        if self.visible {
            if changed && (self.scope.sessions() || self.query.is_empty()) {
                self.request_catalog(cx);
            }
            self.rank(cx);
            if !self.query.is_empty() {
                self.refresh_live(cx);
            }
        }
    }
    pub(crate) fn forget_session(&mut self, id: i64, cx: &mut Context<Self>) {
        Arc::make_mut(&mut self.summaries).retain(|session| session.id != id);
        self.live.remove(&id);
        self.live_revision += 1;
        if self.scope.sessions() {
            // The saved-history query may have opened its SQLite snapshot
            // before deletion. Retire that query as well as the live rows.
            self.refresh(cx);
        }
    }
    fn refresh_live(&mut self, cx: &mut Context<Self>) {
        if !self.visible
            || self.query.is_empty()
            || !self.scope.sessions()
            || self.live_task.is_some()
        {
            return;
        }
        let check = self.cancel.clone();
        self.live_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            let Ok(Some((work, revision))) = this.update(cx, |this, cx| {
                if check.load(Ordering::Acquire) {
                    return None;
                }
                let live = this.live.clone();
                let summaries = this.summaries.clone();
                let query = this.query.clone();
                let flag = this.cancel.clone();
                Some((
                    cx.background_executor().spawn(async move {
                        summaries
                            .iter()
                            .flat_map(|s| {
                                live.get(&s.id)
                                    .map(|rows| search::messages(s, rows, &query, &flag))
                                    .unwrap_or_default()
                            })
                            .collect::<Vec<_>>()
                    }),
                    this.live_revision,
                ))
            }) else {
                return;
            };
            let hits = work.await;
            let _ = this.update(cx, |this, cx| {
                if check.load(Ordering::Acquire) {
                    return;
                }
                this.live_task = None;
                if this.live_revision == revision {
                    this.local = hits;
                    this.rank(cx);
                } else {
                    this.refresh_live(cx);
                }
            });
        }));
    }
    fn request_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog_revision += 1;
        self.start_catalog(cx);
    }

    fn start_catalog(&mut self, cx: &mut Context<Self>) {
        if !self.visible || self.catalog_task.is_some() {
            return;
        }
        let revision = self.catalog_revision;
        let (query, scope) = (self.query.clone(), self.scope);
        let summaries = self.summaries.clone();
        let files = self.files.clone();
        let projects = self.projects.clone();
        let recent = self.recent.clone();
        let work = cx.background_executor().spawn(async move {
            if query.is_empty() {
                return search::recents(&summaries, &files, &recent, &projects, scope);
            }
            let titles = scope.sessions().then(|| search::titles(&summaries, &query));
            let names = scope
                .files()
                .then(|| search::filenames(&files, &query, &recent));
            let projects = matches!(scope, Scope::All | Scope::Projects)
                .then(|| search::projects(&projects, &query));
            search::grouped(
                titles.into_iter().chain(names).chain(projects).flatten(),
                scope,
            )
        });
        self.catalog_task = Some(cx.spawn(async move |this, cx| {
            let hits = work.await;
            let _ = this.update(cx, |this, cx| {
                this.catalog_task = None;
                if !this.visible {
                    return;
                }
                if revision == this.catalog_revision {
                    this.catalog_hits = hits;
                    this.rank(cx);
                } else {
                    this.start_catalog(cx);
                }
            });
        }));
    }

    fn rank(&mut self, cx: &mut Context<Self>) {
        let selected = self.hits.get(self.active).map(|hit| hit.id.clone());
        self.hits = search::grouped(
            self.catalog_hits
                .iter()
                .chain(&self.local)
                .chain(&self.remote)
                .chain(&self.content)
                .cloned(),
            self.scope,
        );
        self.active = selected
            .and_then(|id| self.hits.iter().position(|hit| hit.id == id))
            .unwrap_or(0);
        self.pointer = None;
        cx.notify();
    }
    fn cancel_query(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.debounce = None;
        self.live_task = None;
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.cancel_query();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.local.clear();
        self.remote.clear();
        self.content.clear();
        self.catalog_hits.clear();
        self.error = None;
        self.truncated = (false, false);
        self.sessions_loading = !self.query.is_empty() && self.scope.sessions();
        self.files_loading = !self.query.is_empty() && self.scope.files();
        self.request_catalog(cx);
        self.rank(cx);
        self.scroll.set_offset(point(px(0.), px(0.)));
        if !self.visible || self.query.is_empty() {
            return;
        }
        let check = self.cancel.clone();
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let _ = this.update(cx, |this, cx| {
                if !check.load(Ordering::Acquire) {
                    this.start(cx);
                }
            });
        }));
    }
    fn start(&mut self, cx: &mut Context<Self>) {
        if self.scope.sessions() {
            let store = self.store.clone();
            let query = self.query.clone();
            let flag = self.cancel.clone();
            let check = flag.clone();
            let live = self.live.clone();
            let summaries = self.summaries.clone();
            let revision = self.live_revision;
            let work = cx.background_executor().spawn(async move {
                let mut local = vec![];
                for s in summaries.iter() {
                    if let Some(rows) = live.get(&s.id) {
                        local.extend(search::messages(s, rows, &query, &flag));
                    }
                }
                let saved = locked(&store)
                    .and_then(|db| db.search_reader())
                    .and_then(|reader| match reader {
                        Some(reader) => reader.search_sessions(&query, flag.clone()),
                        None => {
                            locked(&store).and_then(|db| db.search_sessions(&query, flag.clone()))
                        }
                    });
                (local, saved)
            });
            cx.spawn(async move |this, cx| {
                let (local, saved) = work.await;
                let _ = this.update(cx, |this, cx| {
                    if check.load(Ordering::Acquire) {
                        return;
                    }
                    this.sessions_loading = false;
                    match saved {
                        Ok((hits, truncated)) => {
                            this.remote = hits;
                            this.truncated.0 = truncated;
                        }
                        Err(e) => this.error = Some(e),
                    }
                    if this.live_revision == revision {
                        this.local = local;
                    } else {
                        this.refresh_live(cx);
                    }
                    this.rank(cx);
                });
            })
            .detach();
        }
        if self.scope.files() {
            let cwd = self.cwd.clone();
            let query = self.query.clone();
            let flag = self.cancel.clone();
            let check = flag.clone();
            let work = cx.background_executor().spawn(async move {
                search::project::search_project(
                    &std::path::PathBuf::from(&cwd),
                    &search::project::SearchOptions {
                        cwd: cwd.clone(),
                        query: query.clone(),
                        case_sensitive: false,
                        whole_word: false,
                        regex: false,
                        include: None,
                        exclude: None,
                        search_id: String::new(),
                    },
                    &flag,
                )
                .map(|r| (search::content(r.matches, &query), r.truncated))
            });
            cx.spawn(async move |this, cx| {
                let result = work.await;
                let _ = this.update(cx, |this, cx| {
                    if check.load(Ordering::Acquire) {
                        return;
                    }
                    this.files_loading = false;
                    match result {
                        Ok((hits, truncated)) => {
                            this.content = hits;
                            this.truncated.1 = truncated;
                        }
                        Err(e) => this.error = Some(e),
                    }
                    this.rank(cx);
                });
            })
            .detach();
        }
    }
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.input.read(cx).is_composing() || self.hits.is_empty() {
            return;
        }
        self.active = (self.active as isize + delta).rem_euclid(self.hits.len() as isize) as usize;
        self.pointer = None;
        self.scroll
            .scroll_to_item(self.active + usize::from(self.truncated.0 || self.truncated.1));
        cx.notify();
    }
    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.input.read(cx).is_composing() {
            return;
        }
        if let Some(hit) = self.hits.get(ix) {
            cx.emit(Event::Open(hit.target.clone(), self.query.clone()));
        }
    }

    fn pick_hit(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(ix) = self.hits.iter().position(|hit| hit.id == id) {
            self.pick(ix, cx);
        }
    }
}
fn empty_state() -> AnyElement {
    page::empty(
        icon("search", 24.),
        "Search your workspace",
        "Find files, conversations, messages, and projects.",
    )
    .flex_1()
    .min_h_0()
    .into_any_element()
}
impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .hits
            .iter()
            .enumerate()
            .map(|(ix, hit)| {
                let id = hit.id.clone();
                let hover_id = id.clone();
                let mark = match &hit.target {
                    Target::Conversation(_) => provider_icon(&hit.provider, 14.).into_any_element(),
                    Target::Message { .. } => icon("message-square", 14.)
                        .text_color(content(0.55))
                        .into_any_element(),
                    Target::File { path, .. } => vorkspace_ui::file_icons::file_type_icon(
                        &search::project_name(path),
                        false,
                        false,
                        16.,
                    ),
                    Target::Project(path) => crate::project_mark::project_icon(path, 14.),
                };
                let text = StyledText::new(hit.title.clone()).with_highlights(
                    hit.highlights.iter().cloned().map(|range| {
                        (
                            range,
                            HighlightStyle {
                                font_weight: None,
                                color: Some(accent_color()),
                                ..Default::default()
                            },
                        )
                    }),
                );
                page::row(
                    ("app-search-row", ix),
                    format!("{} {}", hit.title, hit.meta),
                    self.active == ix,
                )
                .debug_selector(move || format!("app-search-row-{ix}"))
                .role(Role::ListItem)
                .aria_selected(self.active == ix)
                .h(px(36.))
                .py_0()
                .w_full()
                .flex_shrink_0()
                .flex()
                .gap_2()
                .items_center()
                .text_size(px(page::LABEL_SIZE))
                .line_height(px(page::LABEL_LINE))
                .child(
                    div()
                        .size(px(16.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(mark),
                )
                .child(div().min_w_0().flex_1().truncate().child(text))
                .child(
                    div()
                        .min_w_0()
                        .max_w(relative(0.45))
                        .truncate()
                        .text_size(px(page::META_SIZE))
                        .text_color(page::secondary_ink())
                        .child(hit.meta.clone()),
                )
                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                    if this.pointer.is_some_and(|old| old != event.position)
                        && let Some(current) = this.hits.iter().position(|hit| hit.id == hover_id)
                        && this.active != current
                    {
                        this.active = current;
                        cx.notify();
                    }
                    this.pointer = Some(event.position);
                }))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| this.pick_hit(&id, cx)))
                .into_any_element()
            })
            .collect();
        let loading = self.sessions_loading
            || self.files_loading
            || self.catalog_task.is_some()
            || (self.listing_pending && self.scope.files() && !self.query.is_empty());
        div()
            .id("app-search")
            .key_context("Workspace")
            .debug_selector(|| "app-search".into())
            .aria_label("Search")
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .capture_action(cx.listener(|this, _: &input::Down, _, cx| {
                this.step(1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Up, _, cx| {
                this.step(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input::Submit, _, cx| {
                this.pick(this.active, cx);
                cx.stop_propagation();
            }))
            .on_action(
                cx.listener(|this, _: &vorkspace_ui::actions::DismissMenu, _, cx| {
                    if !this.input.read(cx).is_composing() {
                        cx.emit(Event::Close);
                    }
                    cx.stop_propagation();
                }),
            )
            .child(
                page::toolbar()
                    .id("app-search-header")
                    .debug_selector(|| "app-search-header".into())
                    .child(
                        page::search_frame(&self.input.focus_handle(cx))
                            .flex_1()
                            .child(icon("search", page::ICON_SIZE))
                            .child(div().flex_1().min_w_0().child(self.input.clone())),
                    )
                    .when(loading, |d| {
                        d.child(crate::panel_kit::spinner("app-search-loading", 14.))
                    }),
            )
            .child(
                page::toolbar()
                    .id("app-search-scopes")
                    .debug_selector(|| "app-search-scopes".into())
                    .role(Role::TabList)
                    .aria_label("Search scopes")
                    .children(
                        Scope::ALL
                            .into_iter()
                            .enumerate()
                            .map(|(ix, (scope, label))| {
                                page::tab(("app-search-scope", ix), label, self.scope == scope)
                                    .debug_selector(move || format!("app-search-scope-{ix}"))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_scope(scope, cx);
                                    }))
                            }),
                    ),
            )
            .child(if self.query.is_empty() && self.hits.is_empty() {
                empty_state()
            } else {
                div()
                    .id("app-search-results")
                    .role(Role::ListBox)
                    .aria_label("Search results")
                    .flex_1()
                    .min_h_0()
                    .p(px(6.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .when(self.query.is_empty(), |d| {
                        d.child(
                            div()
                                .px_2p5()
                                .pt_1()
                                .pb_1p5()
                                .text_size(px(page::META_SIZE))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(page::secondary_ink())
                                .child("Recent"),
                        )
                    })
                    .when(self.truncated.0 || self.truncated.1, |d| {
                        d.child(
                            div()
                                .px_2p5()
                                .py_1()
                                .text_size(px(page::META_SIZE))
                                .text_color(page::secondary_ink())
                                .child("Results limited to the first matches"),
                        )
                    })
                    .children(rows)
                    .when(self.hits.is_empty() && !loading, |d| {
                        d.child(
                            div()
                                .p(px(12.))
                                .text_size(px(12.))
                                .line_height(px(page::META_LINE))
                                .text_color(page::secondary_ink())
                                .child(self.error.clone().unwrap_or_else(|| "No results".into())),
                        )
                    })
                    .into_any_element()
            })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod screenshots;
