//! Bot workspaces: one runtime per identity, shared by the rail, pages,
//! overview and settings.
pub(crate) mod avatar;
pub(crate) mod cards;
mod changes;
mod channels;
pub(crate) mod color_picker;
mod creation;
mod files;
mod filters;
mod header;
pub(crate) use header::{ChatHeader, chat_surface};
#[cfg(test)]
mod header_tests;
mod host;
mod html;
mod memory_ui;
mod notifications;
mod overview;
mod panel_stack;
mod panels;
mod peers;
mod scheduler;
mod search;
#[cfg(test)]
mod tests;
mod work;

use crate::{
    conversation::{BotSubmitted, Conversation, SessionChanged},
    input::{self, Input},
    popover::{Anchor, AnchorSlot, Menu, MenuEvent, menu_popover, popover},
    runtime::{Database, locked},
    shell::Shell,
    theme::*,
};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, Focusable, MouseButton, Role, SharedString,
    Subscription, WeakEntity, Window, div, prelude::*, px, rgb,
};
use panels::{OpenPicker, OverviewTab, Route, SettingsSection};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};
use vorkspace_core::{
    agent::{AccessMode, Settings},
    bot::{
        self, Bot, Message, Roster,
        files::AgentFiles,
        habits::{self, Habit},
    },
    provider::Kind,
    store::Session,
};
pub(crate) struct Changed;
pub(crate) struct TabOpened(pub String);
pub(crate) struct ShowDetails(pub String);
pub(crate) use notifications::{NotificationChanged, NotificationFinished};
#[derive(Clone, PartialEq)]
pub(crate) struct OverviewChrome {
    active: Option<String>,
    creating: bool,
    tab: OverviewTab,
    routed: bool,
    counts: (usize, usize, usize, usize),
}
struct RunningHabit {
    bot: String,
    habit: String,
    view: Entity<Conversation>,
    started: u64,
    launched: u64,
    failure: Option<String>,
    waiting_since: Option<u64>,
    cards: Vec<serde_json::Value>,
    mailbox: Vec<String>,
}
/// A minion session's context and app environment.
pub(crate) type MinionAttachment = (
    vorkspace_core::agent::MinionContext,
    std::collections::HashMap<String, String>,
);
/// Minion sessions by ID, shared with the shell so it can attach them to
/// their views without reading `Bots`, which may be mid-update.
pub(crate) type MinionRegistry =
    std::rc::Rc<std::cell::RefCell<std::collections::HashMap<i64, MinionAttachment>>>;

pub(crate) struct Bots {
    pub(crate) minion_registry: MinionRegistry,
    channels: channels::ChannelUi,
    parent_window: Option<gpui::AnyWindowHandle>,
    owner: WeakEntity<Shell>,
    store: Database,
    root: PathBuf,
    prefs: crate::settings::Preferences,
    pub(crate) roster: Roster,
    pub(crate) active: Option<String>,
    chats: HashMap<String, Entity<Conversation>>,
    bridge: Option<bot::bridge::Server>,
    observations: Vec<Subscription>,
    _bridge_task: Option<gpui::Task<()>>,
    _clock: Option<gpui::Task<()>>,
    writes: Option<gpui::Task<()>>,
    loading_chats: HashSet<String>,
    awaiting_creation: HashSet<String>,
    work: work::Cache,
    pub(crate) error: Option<String>,
    notice_region: AnchorSlot,
    pub(crate) details: Entity<overview::Overview>,
    overview_tab: OverviewTab,
    /// The current Bot's to-do board, behind its To-dos tab.
    todo_board: Option<Entity<crate::todos::TodoBoardView>>,
    overview_reveal: Option<crate::project_tabs::TabReveal<OverviewTab>>,
    route: Option<Route>,
    settings: Option<SettingsSection>,
    creation: Option<creation::Creation>,
    settings_focus: gpui::FocusHandle,
    fields: HashMap<String, Entity<Input>>,
    memory_editor: Option<memory_ui::MemoryEditor>,
    memory_editor_revision: u64,
    memory_pending: HashSet<memory_ui::MemoryTarget>,
    memory_topics: Vec<memory_ui::MemoryTopic>,
    memory_loading: bool,
    memory_load_error: Option<String>,
    field_blurs: HashSet<gpui::EntityId>,
    file_versions: HashMap<String, (String, String)>,
    saving_files: HashSet<String>,
    file_load_revision: u64,
    files: Option<AgentFiles>,
    habits: Vec<Habit>,
    habit_ui: scheduler::HabitUi,
    picker: Option<OpenPicker>,
    settings_anchor: AnchorSlot,
    model_anchor: AnchorSlot,
    projects_anchor: AnchorSlot,
    access_anchor: AnchorSlot,
    menu: Option<(String, Entity<Menu>, Vec<Subscription>)>,
    menu_anchor: Anchor,
    documents: HashMap<String, Entity<crate::transcript::TranscriptView>>,
    html_documents: HashMap<String, Entity<html::Html>>,
    project_changes: changes::ProjectChanges,
    review: Option<Entity<crate::review::ReviewPane>>,
    rail_ids: Vec<String>,
    running: HashMap<i64, RunningHabit>,
    active_run: Option<i64>,
    completion_work: HashSet<(String, u64)>,
    /// The open background dialog, centred in the window it opened in.
    background: Option<(
        Entity<crate::project_settings::BackgroundDialog>,
        Subscription,
    )>,
    backgrounds: HashMap<String, Entity<crate::project_background::ProjectBackground>>,
    drag: Option<(String, gpui::Point<gpui::Pixels>, Option<String>)>,
    rail_bounds: HashMap<String, AnchorSlot>,
    skip_click: bool,
    scheduling: bool,
    persisted_messages: HashSet<String>,
    failed_reports: HashSet<String>,
    inflight: HashMap<String, Message>,
    /// Claims submitted by this manager, retained until the message leaves its queue.
    dispatch_claims: HashSet<String>,
    /// Outbox messages the user queued: they wait for the running reply
    /// instead of being steered into it.
    held: HashSet<String>,
    card_refresh: HashMap<String, u64>,
    session_filters: filters::Filters,
    session_filter_anchor: AnchorSlot,
}
impl EventEmitter<Changed> for Bots {}
impl EventEmitter<TabOpened> for Bots {}
impl EventEmitter<ShowDetails> for Bots {}
impl Bots {
    pub(crate) fn conversation_for_session(
        &self,
        id: i64,
        cx: &App,
    ) -> Option<Entity<Conversation>> {
        self.chats
            .values()
            .chain(self.channels.views.values())
            .find(|view| view.read(cx).summary.id == id)
            .cloned()
    }
    pub(crate) fn resource_sessions(
        &self,
        cx: &App,
    ) -> Vec<(i64, crate::resource_pressure::SessionInfo)> {
        self.chats
            .values()
            .chain(self.running.values().map(|r| &r.view))
            .chain(self.channels.views.values())
            .map(|v| {
                let s = &v.read(cx).summary;
                (
                    s.id,
                    crate::resource_pressure::SessionInfo {
                        title: s.title.clone().into(),
                        provider: s.provider.clone(),
                    },
                )
            })
            .collect()
    }
    pub(crate) fn stop_session(&mut self, id: i64, cx: &mut Context<Self>) {
        let channel = self
            .channels
            .views
            .iter()
            .find(|(_, v)| v.read(cx).summary.id == id)
            .map(|(key, _)| key.clone());
        if let Some((channel, bot)) = channel {
            let running = self
                .channels
                .state
                .get(&channel)
                .and_then(|c| {
                    c.deliveries
                        .iter()
                        .find(|d| d.bot == bot && d.state == "running")
                })
                .map(|d| d.id.clone());
            if let Some(delivery) = running {
                self.stop_channel_delivery(&channel, &delivery, cx);
                return;
            }
        }

        if let Some(view) = self
            .chats
            .values()
            .chain(self.running.values().map(|r| &r.view))
            .chain(self.channels.views.values())
            .find(|v| v.read(cx).summary.id == id)
        {
            view.update(cx, |v, cx| v.stop_if_busy(cx));
        }
    }
    pub(crate) fn bind_window(&mut self, window: &Window) {
        self.parent_window = Some(window.window_handle());
    }
    pub(super) fn open_workspace(&self, id: i64, window: &Window, cx: &mut Context<Self>) {
        let owner = self.owner.clone();
        let handle = window.window_handle();
        let app: &mut App = cx;
        app.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = owner.update(cx, |owner, cx| owner.open(id, window, cx));
            });
        });
    }
    pub(crate) fn move_rail_drag(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some((_, start, over)) = &mut self.drag {
            if event.pressed_button != Some(MouseButton::Left) {
                self.finish_reorder(cx);
                return;
            }
            if (event.position - *start).magnitude() > 5. {
                self.skip_click = true;
                *over = self.rail_bounds.iter().find_map(|(id, s)| {
                    s.get()
                        .filter(|b| b.contains(&event.position))
                        .map(|_| id.clone())
                });
            }
        }
    }
    pub(crate) fn visible_idle_sessions(&self, cx: &App) -> Vec<i64> {
        self.chat()
            .into_iter()
            .map(|chat| chat.read(cx).summary.id)
            .collect()
    }
    pub(crate) fn hide_html(&self, cx: &App) {
        for view in self.html_documents.values() {
            view.read(cx).hide();
        }
    }
    pub(crate) fn sidebar_preferences(
        &mut self,
        id: &str,
        show: bool,
        folders: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(m) = self.roster.get_mut(id) {
            m.show_started_sessions_in_sidebar = show;
            m.use_sidebar_folders = folders;
        }
        let sessions = self
            .roster
            .get(id)
            .map(|m| m.launches.iter().map(|l| l.session_id).collect::<Vec<_>>())
            .unwrap_or_default();
        for session in sessions {
            self.group_launch(id, session, cx);
        }
        self.configure_chat(id, cx);
        self.save(cx);
    }
    pub(crate) fn new(
        store: Database,
        owner: WeakEntity<Shell>,
        prefs: crate::settings::Preferences,
        active: Option<String>,
        notice_region: AnchorSlot,
        cx: &mut Context<Self>,
    ) -> Self {
        bot::set_minions_enabled(prefs.bots && prefs.minions);
        let (roster, root, roster_error) = match locked(&store) {
            Ok(db) => match Roster::load(&db) {
                Ok(roster) => (roster, bot::root(&db), None),
                Err(error) => (Roster::default(), bot::root(&db), Some(error)),
            },
            Err(error) => (
                Roster::default(),
                std::env::temp_dir().join(bot::new_id()),
                Some(error),
            ),
        };
        let (bridge, rx) = if cx.is_test() {
            (None, None)
        } else {
            match bot::bridge::Server::start() {
                Ok((server, rx)) => (Some(server), Some(rx)),
                Err(_) => (None, None),
            }
        };
        let persisted_messages = roster
            .bots
            .iter()
            .flat_map(|m| m.outbox.iter().map(|q| q.id.clone()))
            .collect();
        let held = roster.held_messages.clone();
        let session_filters = locked(&store)
            .ok()
            .and_then(|s| s.setting("bot-session-filters").ok().flatten())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let manager = cx.entity();
        let details = cx.new(|cx| overview::Overview::new(manager, cx));
        let mut channel_state =
            locked(&store).and_then(|db| vorkspace_core::bot::channels::Channels::load(&db));
        if let Ok(state) = &mut channel_state {
            state.recover();
        }
        let channel_error = channel_state.as_ref().err().cloned();
        let mut this = Self {
            channels: channels::ChannelUi {
                state: channel_state.unwrap_or_default(),
                read_through: locked(&store)
                    .ok()
                    .and_then(|db| db.setting(channels::READ_KEY).ok().flatten())
                    .and_then(|raw| serde_json::from_str(&raw).ok())
                    .unwrap_or_default(),
                load_error: channel_error.clone(),
                ..Default::default()
            },
            parent_window: None,
            owner,
            store,
            root,
            prefs,
            roster,
            active: None,
            chats: HashMap::new(),
            bridge,
            observations: Vec::new(),
            _bridge_task: None,
            _clock: None,
            writes: None,
            loading_chats: HashSet::new(),
            awaiting_creation: HashSet::new(),
            work: work::Cache::default(),
            error: roster_error.or(channel_error),
            notice_region,
            details,
            minion_registry: Default::default(),
            overview_tab: OverviewTab::Sessions,
            todo_board: None,
            overview_reveal: None,
            route: None,
            settings: None,
            creation: None,
            settings_focus: cx.focus_handle(),
            fields: HashMap::new(),
            memory_editor: None,
            memory_editor_revision: 0,
            memory_pending: HashSet::new(),
            memory_topics: Vec::new(),
            memory_loading: false,
            memory_load_error: None,
            field_blurs: HashSet::new(),
            file_versions: HashMap::new(),
            saving_files: HashSet::new(),
            file_load_revision: 0,
            files: None,
            habits: Vec::new(),
            habit_ui: scheduler::HabitUi::default(),
            picker: None,
            settings_anchor: AnchorSlot::default(),
            model_anchor: AnchorSlot::default(),
            projects_anchor: AnchorSlot::default(),
            access_anchor: AnchorSlot::default(),
            menu: None,
            menu_anchor: Anchor::Point(gpui::Point::default()),
            documents: HashMap::new(),
            html_documents: HashMap::new(),
            project_changes: Default::default(),
            review: None,
            rail_ids: Vec::new(),
            running: HashMap::new(),
            active_run: None,
            completion_work: HashSet::new(),
            background: None,
            backgrounds: HashMap::new(),
            drag: None,
            rail_bounds: HashMap::new(),
            skip_click: false,
            scheduling: false,
            persisted_messages,
            failed_reports: HashSet::new(),
            inflight: HashMap::new(),
            dispatch_claims: HashSet::new(),
            held,
            card_refresh: HashMap::new(),
            session_filters,
            session_filter_anchor: AnchorSlot::default(),
        };
        if let Some(rx) = rx {
            this._bridge_task = Some(cx.spawn(async move |this, cx| {
                while let Ok(call) = rx.recv().await {
                    if this
                        .update_in(cx, |this, window, cx| this.app_call(call, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        this._clock = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(10))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| this.tick(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        if this.prefs.bots
            && let Some(id) = active.filter(|id| this.roster.get(id).is_some())
        {
            this.active = Some(id.clone());
            this.ensure_chat(&id, cx);
            this.reload_files(&id, cx);
        }
        this.resume_peer_queues(cx);
        this.sync_minions();
        this
    }
    pub(crate) fn is_bot_session(&self, id: i64) -> bool {
        self.roster.hidden_sessions.contains(&id)
            || self.roster.of_session(id).is_some()
            || self
                .roster
                .bots
                .iter()
                .any(|m| m.habit_sessions.contains(&id))
    }
    pub(crate) fn protected_app_target(&self, id: i64, cx: &App) -> bool {
        // Sidebar visibility is a display choice. Only internal identities
        // (and their active owners) are protected from ordinary app controls.
        self.is_bot_session(id)
            || self.running.contains_key(&id)
            || self
                .channels
                .views
                .values()
                .any(|view| view.read(cx).summary.id == id)
    }
    pub(crate) fn app_project_allowed(&self, caller: i64, project: &str, cx: &App) -> bool {
        self.roster
            .of_session(caller)
            .or_else(|| {
                self.running
                    .get(&caller)
                    .and_then(|run| self.roster.get(&run.bot))
            })
            .or_else(|| {
                self.channels
                    .views
                    .iter()
                    .find(|(_, view)| view.read(cx).summary.id == caller)
                    .and_then(|((_, bot), _)| self.roster.get(bot))
            })
            .is_some_and(|bot| bot.works_on(project))
    }
    pub(crate) fn hidden_session(&self, id: i64) -> bool {
        // Minions always show under their project, like sessions.
        if self.roster.of_minion(id).is_some() {
            return false;
        }
        self.is_bot_session(id)
            || self.roster.bots.iter().any(|m| {
                !m.show_started_sessions_in_sidebar && m.launches.iter().any(|l| l.session_id == id)
            })
    }
    pub(crate) fn set_preferences(
        &mut self,
        prefs: crate::settings::Preferences,
        cx: &mut Context<Self>,
    ) {
        let was = self.prefs.bots;
        self.prefs = prefs;
        bot::set_minions_enabled(self.prefs.bots && self.prefs.minions);
        self.sync_minions();
        if was && !self.prefs.bots {
            self.active = None;
            self.creation = None;
            self.picker = None;
        }
        if !self.prefs.channels {
            self.close_classic_channels(cx);
        }
        cx.notify();
    }
    pub(crate) fn overview_chrome(&self) -> OverviewChrome {
        let counts = self.current().map_or((0, 0, 0, 0), |bot| {
            (
                bot.launches.len(),
                bot.documents.len(),
                self.habits.len(),
                self.project_changes.count(&bot.id),
            )
        });
        OverviewChrome {
            active: self.active.clone(),
            creating: self.creating(),
            tab: self.overview_tab,
            routed: self.route.is_some(),
            counts,
        }
    }
    fn show_overview(&self, cx: &mut Context<Self>) {
        if let Some(id) = self.active.clone() {
            cx.emit(ShowDetails(id));
        }
    }
    pub(crate) fn current(&self) -> Option<&Bot> {
        self.active.as_deref().and_then(|id| self.roster.get(id))
    }
    pub(crate) fn chat(&self) -> Option<Entity<Conversation>> {
        self.active
            .as_ref()
            .and_then(|id| self.chats.get(id))
            .cloned()
    }
    /// Open a Bot's chat in the background for an application action.
    pub(crate) fn chat_for_action(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Conversation>> {
        self.ensure_chat(id, cx);
        self.chats.get(id).cloned()
    }
    /// Queue an application message through the Bot's ordinary outbox.
    pub(crate) fn send_message(
        &mut self,
        id: &str,
        text: String,
        attachments: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.roster.get(id).is_none() {
            return Err("Bot is no longer available".into());
        }
        self.ensure_chat(id, cx);
        self.enqueue(id, text, attachments, false, cx);
        Ok(())
    }
    pub(crate) fn account_changed(
        &mut self,
        kind: Kind,
        id: &str,
        removed: bool,
        cx: &mut Context<Self>,
    ) {
        for view in self
            .chats
            .values()
            .chain(self.running.values().map(|run| &run.view))
            .chain(self.channels.views.values())
        {
            if view.read(cx).summary.provider == kind.id()
                && view.read(cx).settings.account.id == id
            {
                view.update(cx, |view, cx| {
                    if removed {
                        view.account_unavailable(cx)
                    } else {
                        view.account_authenticated(cx)
                    }
                });
            }
        }
    }
    /// Peer senders resolve to avatars by name against the current roster.
    fn sync_peer_looks(&self, cx: &mut Context<Self>) {
        let looks = crate::transcript::peer_looks(&self.roster.bots);
        for view in self.chats.values() {
            let transcript = view.read(cx).transcript.clone();
            transcript.update(cx, |t, cx| t.set_peer_looks(looks.clone(), cx));
        }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.sync_peer_looks(cx);
        cx.emit(Changed);
        cx.notify();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        self.sync_minions();
        let live: HashSet<_> = self
            .roster
            .bots
            .iter()
            .flat_map(|bot| bot.outbox.iter().map(|message| message.id.clone()))
            .collect();
        self.roster.held_messages.retain(|id| live.contains(id));
        self.roster.message_modes.retain(|id, _| live.contains(id));
        self.held.retain(|id| live.contains(id));
        self.dispatch_claims.retain(|id| live.contains(id));
        let roster = self.roster.clone();
        let store = self.store.clone();
        let queued = roster
            .bots
            .iter()
            .map(|m| (m.id.clone(), m.outbox.clone()))
            .collect::<Vec<_>>();
        let previous = self.writes.take();
        self.writes = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx.background_executor().spawn(async move {
                let db = locked(&store)?;
                roster.save(&db)
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Err(error) => {
                    for (id, messages) in &queued {
                        if let Some(view) = this.chats.get(id) {
                            for message in messages {
                                view.update(cx, |view, cx| {
                                    view.bot_draft_outbox_failed(&message.id, &error, cx)
                                });
                            }
                        }
                    }
                    this.error = Some(error);
                    cx.notify();
                }
                Ok(()) => {
                    for (id, messages) in queued {
                        if this.awaiting_creation.remove(&id) {
                            this.ensure_chat(&id, cx);
                        }
                        if let Some(view) = this.chats.get(&id) {
                            for message in &messages {
                                if !this.roster.get(&id).is_some_and(|bot| {
                                    bot.outbox.iter().any(|current| {
                                        current.id == message.id
                                            && current.text == message.text
                                            && current.attachments == message.attachments
                                    })
                                }) {
                                    continue;
                                }
                                view.update(cx, |view, cx| {
                                    view.bot_draft_outbox_saved(message, cx)
                                });
                            }
                        }
                        this.persisted_messages
                            .extend(messages.into_iter().map(|message| message.id));
                        this.dispatch(&id, cx);
                    }
                    this.persisted_messages.retain(|id| {
                        this.roster
                            .bots
                            .iter()
                            .any(|bot| bot.outbox.iter().any(|m| &m.id == id))
                    });
                }
            });
        }));
        self.changed(cx);
    }
    fn ensure_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.awaiting_creation.contains(id) {
            return;
        }
        if self.chats.contains_key(id) || !self.loading_chats.insert(id.into()) {
            return;
        }
        let Some(bot) = self.roster.get(id).cloned() else {
            return;
        };
        let store = self.store.clone();
        let prefs = self.prefs.clone();
        let id = id.to_string();
        let root = self.root.clone();
        let work = cx.background_executor().spawn(async move {
            let db = locked(&store)?;
            if let Some(missing) = &bot.unavailable_session {
                return Err(format!("This Bot's saved conversation ({missing}) is unavailable. Its identity and history have been preserved."));
            }
            if let Some(session) = bot.session_id {
                let summary = db.bot_session(session)?
                    .ok_or("This Bot's saved conversation is unavailable. Reset its conversation explicitly to start a new one.")?;
                let delivered = bot
                    .outbox
                    .iter()
                    .filter(|q| {
                        db.setting(&format!("bot-delivered:{}:{}", summary.id, q.id))
                            .ok()
                            .flatten()
                            .is_some()
                    })
                    .map(|q| q.id.clone())
                    .collect::<Vec<_>>();
                return Ok::<_, String>((summary, None, delivered));
            }
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| ".".into());
            let provider = match &bot.initial_configuration {
                Some(config) => Kind::parse(&config.provider).ok_or("This Bot's configured provider is unavailable.")?,
                None => prefs.default_provider.as_deref().and_then(Kind::parse).unwrap_or(Kind::Codex),
            };
            if bot.initial_configuration.is_some() {
                bot::load_files(&root, &bot)?;
            }
            let mut summary = db.create(provider.id(), &home)?;
            summary.title = bot.label();
            db.title(summary.id, &summary.title)?;
            let settings = bot.initial_configuration.as_ref().map(|config| config.settings.clone()).unwrap_or_else(|| Settings {
                access: AccessMode::Auto,
                model: prefs.default_models.get(provider.id()).cloned(),
                ..Default::default()
            });
            db.configure_agent(summary.id, provider.id(), &settings)?;
            Ok((summary, Some(settings), Vec::new()))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.loading_chats.remove(&id);
                let (summary, settings, delivered) = match result {
                    Ok(value) => value,
                    Err(e) => {
                        this.error = Some(e);
                        this.changed(cx);
                        return;
                    }
                };
                if !this.roster.hidden_sessions.contains(&summary.id) {
                    this.roster.hidden_sessions.push(summary.id);
                }
                let Some(bot) = this.roster.get_mut(&id) else {
                    return;
                };
                bot.session_id = Some(summary.id);
                bot.initial_configuration = None;
                bot.outbox.retain_mut(|q| {
                    if !delivered.contains(&q.id) { return true; }
                    if q.peer.is_some() {
                        q.error = Some("Interrupted before the peer reply was recorded. Inspect the conversation before retrying.".into());
                        true
                    } else { false }
                });
                let bot = bot.clone();
                let page_chat = this.active.as_deref() == Some(id.as_str());
                let view = cx.new(|cx| Conversation::new(summary, this.store.clone(), cx));
                let root = this.root.clone();
                let env = this
                    .bridge
                    .as_ref()
                    .map(|b| b.environment(view.read(cx).summary.id))
                    .unwrap_or_default();
                view.update(cx, |view, cx| {
                    view.allow_storage_paging = page_chat;
                    view.configure_bot(bot, root, env, cx);
                    for message in this.roster.get(&id).into_iter().flat_map(|bot| &bot.outbox) {
                        if this.persisted_messages.contains(&message.id) {
                            view.bot_draft_outbox_saved(message, cx);
                        }
                    }
                    if let Some(settings) = settings {
                        let config = view.settings.bot.take();
                        let env = std::mem::take(&mut view.settings.environment);
                        view.install_transfer(settings, None, cx);
                        view.settings.bot = config;
                        view.settings.environment = env;
                    }
                    view.load(cx);
                });
                let source = id.clone();
                this.observations.push(cx.subscribe(
                    &view,
                    move |this, _, event: &BotSubmitted, cx| {
                        this.enqueue_held(
                            &source,
                            event.text.clone(),
                            event
                                .attachments
                                .iter()
                                .map(|p| p.to_string_lossy().to_string())
                                .collect(),
                            false,
                            event.queue,
                            event.draft_id.clone(),
                            event.mode.clone(),
                            event.prepared_prompt.clone(),
                            cx,
                        )
                    },
                ));
                let source = id.clone();
                this.observations.push(cx.subscribe(
                    &view,
                    move |this, _, _: &SessionChanged, cx| {
                        this.chat_changed(&source, cx);
                    },
                ));
                let source = id.clone();
                this.observations.push(cx.subscribe(
                    &view,
                    move |this, _, _: &crate::conversation::UsageResumed, cx| {
                        let empty = this.roster.get(&source).is_none_or(|m| m.outbox.is_empty());
                        if empty {
                            this.enqueue(
                                &source,
                                "Continue from where you left off.".into(),
                                Vec::new(),
                                false,
                                cx,
                            );
                        } else {
                            if let Some(bot) = this.roster.get_mut(&source) {
                                for message in &mut bot.outbox {
                                    // Usage recovery may be automatic. Only an
                                    // explicit Retry can authorize a new peer
                                    // submission after uncertain external effects.
                                    if message.peer.is_none() {
                                        message.error = None;
                                    }
                                }
                            }
                            this.save(cx);
                            this.configure_chat(&source, cx);
                        }
                    },
                ));
                let source = id.clone();
                this.observations
                    .push(cx.observe(&view, move |this, _, cx| {
                        if this.active.as_ref() == Some(&source) {
                            cx.notify();
                        }
                        this.dispatch(&source, cx);
                        if this.channels.state.channels.iter().any(|c| {
                            !this.channels.views.contains_key(&(c.id.clone(), source.clone()))
                                && c.deliveries.iter().any(|d| d.bot == source && d.state == "queued")
                        }) { this.channel_tick(cx); }
                    }));
                let source = id.clone();
                this.observations.push(cx.subscribe(
                    &view,
                    move |this, _, event: &crate::conversation::BotSteerFailed, cx| {
                        this.steer_failed(&source, &event.id, &event.error, cx)
                    },
                ));
                let source = id.clone();
                this.observations.push(cx.subscribe(
                    &view,
                    move |this, _, event: &crate::conversation::BotQueueChanged, cx| {
                        this.queue_changed(&source, &event.id, event.remove, cx)
                    },
                ));
                let transcript = view.read(cx).transcript.clone();
                let target = view.clone();
                let source = id.clone();
                this.observations.push(cx.subscribe(
                    &transcript,
                    move |this, _, event: &crate::transcript::BotResponseAction, cx| match event {
                        crate::transcript::BotResponseAction::Queue { id, remove } => {
                            this.queue_changed(&source, id, *remove, cx)
                        }
                        crate::transcript::BotResponseAction::Quote { text, window } => {
                            let target = target.clone();
                            let text = text.clone();
                            let handle = *window;
                            let app: &mut App = cx;
                            app.defer(move |cx| {
                                let _ = handle.update(cx, |_, window, cx| {
                                    target.update(cx, |v, cx| v.bot_quote(&text, window, cx))
                                });
                            });
                        }
                    },
                ));
                let source = id.clone();
                this.observations.push(cx.subscribe(
                    &transcript,
                    move |this, _, event: &cards::CardAction, cx| {
                        this.card_action(&source, &event.0, cx)
                    },
                ));
                this.chats.insert(id.clone(), view);
                this.sync_peer_looks(cx);
                if let Some(handle) = this.parent_window {
                    let owner = cx.weak_entity();
                    let id = id.clone();
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.active.as_deref() == Some(&id)
                                    && this.settings_focus.is_focused(window)
                                {
                                    this.focus_active(window, cx);
                                }
                            });
                        });
                    });
                }
                this.save(cx);
                this.dispatch(&id, cx);
                this.channel_tick(cx);
            });
        })
        .detach();
    }
    /// Retry clears a queued message's error; discard drops it.
    fn queue_changed(&mut self, source: &str, id: &str, remove: bool, cx: &mut Context<Self>) {
        let held = self.held.remove(id);
        self.roster.held_messages.remove(id);
        let mode = self.roster.message_modes.remove(id);
        let mut replacement = None;
        if let Some(bot) = self.roster.get_mut(source) {
            if remove {
                bot.outbox.retain(|m| m.id != id);
            } else if let Some(message) = bot.outbox.iter_mut().find(|m| m.id == id) {
                // A retry is a new explicit submission, never a replay of the
                // provider receipt from a failed or interrupted turn or steer.
                message.id = if message.peer.is_some() {
                    format!("peer:retry:{}", bot::new_id())
                } else {
                    bot::new_id()
                };
                message.error = None;
                replacement = Some(message.id.clone());
            }
        }
        if let Some(id) = replacement {
            if held {
                self.held.insert(id.clone());
                self.roster.held_messages.insert(id.clone());
            }
            if let Some(mode) = mode {
                self.roster.message_modes.insert(id, mode);
            }
        }
        self.save(cx);
        self.configure_chat(source, cx);
    }
    fn configure_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        self.refresh_work(id, cx);
        if let (Some(bot), Some(view)) = (self.roster.get(id).cloned(), self.chats.get(id).cloned())
        {
            let env = self
                .bridge
                .as_ref()
                .map(|b| b.environment(view.read(cx).summary.id))
                .unwrap_or_default();
            let root = self.root.clone();
            view.update(cx, |view, cx| view.configure_bot(bot, root, env, cx));
        }
    }
    fn enqueue(
        &mut self,
        id: &str,
        text: String,
        attachments: Vec<String>,
        internal: bool,
        cx: &mut Context<Self>,
    ) {
        self.enqueue_held(id, text, attachments, internal, false, None, None, None, cx);
    }
    #[allow(clippy::too_many_arguments)]
    fn enqueue_held(
        &mut self,
        id: &str,
        text: String,
        attachments: Vec<String>,
        internal: bool,
        hold: bool,
        draft_id: Option<String>,
        mode: Option<String>,
        prepared_prompt: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.roster.get(id).is_none() {
            if let (Some(draft_id), Some(view)) = (draft_id.as_deref(), self.chats.get(id)) {
                view.update(cx, |view, cx| {
                    view.bot_draft_outbox_failed(draft_id, "Bot is no longer available", cx)
                });
            }
            return;
        }
        if let Some(bot) = self.roster.get_mut(id) {
            let message = draft_id.unwrap_or_else(bot::new_id);
            if let Some(queued) = bot.outbox.iter().find(|queued| queued.id == message) {
                if queued.text != text
                    || queued.attachments != attachments
                    || queued.prepared_prompt != prepared_prompt
                {
                    if let Some(view) = self.chats.get(id) {
                        view.update(cx, |view, cx| {
                            view.bot_draft_outbox_failed(
                                &message,
                                "A different version is already queued; reopen the conversation",
                                cx,
                            )
                        });
                    }
                    return;
                }
                self.save(cx);
                return;
            }
            bot.outbox.push(Message {
                peer: None,
                id: message.clone(),
                text,
                prepared_prompt,
                sent_at: Some(vorkspace_core::time::now_ms()),
                internal,
                error: None,
                attachments,
            });
            // Marked before dispatch, which may steer at once.
            if hold || mode.is_some() {
                self.held.insert(message.clone());
                self.roster.held_messages.insert(message.clone());
            }
            if let Some(mode) = mode {
                self.roster.message_modes.insert(message, mode);
            }
            self.save(cx);
            self.configure_chat(id, cx);
            self.dispatch(id, cx);
        }
    }
    fn dispatch(&mut self, id: &str, cx: &mut Context<Self>) {
        let delivered = self
            .chats
            .get(id)
            .and_then(|v| v.read(cx).bot_delivered())
            .map(str::to_owned);
        if let Some(message) = delivered
            && let Some(m) = self.roster.get_mut(id)
            && let Some(index) = m
                .outbox
                .iter()
                .position(|q| q.id == message && q.peer.is_none())
        {
            let sent = m.outbox.remove(index);
            self.held.remove(&sent.id);
            self.roster.held_messages.remove(&sent.id);
            self.roster.message_modes.remove(&sent.id);
            self.save(cx);
            self.configure_chat(id, cx);
        }

        if self.inflight.contains_key(id) {
            self.steer(id, cx);
            return;
        }

        if self
            .roster
            .get(id)
            .is_some_and(|bot| !bot.pending_reports.is_empty())
            && let Some(view) = self.chats.get(id)
        {
            view.update(cx, |view, cx| {
                if view.needs_history_restore() {
                    view.load(cx);
                }
            });
        }
        if let Some(view) = self
            .chats
            .get(id)
            .cloned()
            .filter(|v| !self.failed_reports.contains(id) && v.read(cx).ready_for_external(cx))
        {
            let reports = self
                .roster
                .get_mut(id)
                .map(|m| std::mem::take(&mut m.pending_reports))
                .unwrap_or_default();
            if !reports.is_empty() {
                let mut failed = Vec::new();
                for report in reports {
                    if self.failed_reports.contains(id) {
                        failed.push(report);
                        continue;
                    }
                    let (role, text) = report
                        .strip_prefix("bot-card:")
                        .map(|s| ("bot-card", s.to_string()))
                        .unwrap_or_else(|| ("assistant", report.clone()));
                    if let Err(error) = view.update(cx, |v, cx| v.bot_post(role, text, cx)) {
                        self.error = Some(error);
                        self.failed_reports.insert(id.into());
                        failed.push(report);
                    }
                }
                if let Some(bot) = self.roster.get_mut(id) {
                    bot.pending_reports.extend(failed);
                }
                self.save(cx);
            }
        }
        let Some(message) = self
            .roster
            .get(id)
            .and_then(|m| m.outbox.first())
            .cloned()
            .filter(|m| m.error.is_none() && self.persisted_messages.contains(&m.id))
        else {
            return;
        };
        let Some(view) = self.chats.get(id).cloned() else {
            return;
        };
        if view.read(cx).busy {
            self.steer(id, cx);
            return;
        }
        let Some(token) = self.claim_dispatch(id, &message, cx) else {
            return;
        };
        let mode = self.roster.message_modes.get(&message.id).cloned();
        if view.update(cx, |v, cx| {
            v.bot_dispatch_with_mode(&message, mode.as_deref(), cx)
        }) {
            self.dispatch_claims.insert(message.id.clone());
            self.inflight.insert(id.into(), message);
        } else {
            self.release_unsubmitted_claim(id, &message.id, &token, cx);
        }
    }
    fn claim_dispatch(
        &mut self,
        bot: &str,
        message: &Message,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if self.dispatch_claims.contains(&message.id) {
            return None;
        }
        let token = bot::new_id();
        let result =
            locked(&self.store).and_then(|store| store.claim_bot_message(bot, message, &token));
        match result {
            Ok(true) => Some(token),
            Ok(false) => {
                if let Some(queued) = self
                    .roster
                    .get_mut(bot)
                    .and_then(|bot| bot.outbox.iter_mut().find(|item| item.id == message.id))
                {
                    queued.error = Some("Delivery is uncertain or another window is sending this message. Inspect the conversation before choosing Retry or Discard.".into());
                }
                self.configure_chat(bot, cx);
                None
            }
            Err(error) => {
                self.error = Some(format!("Could not claim Bot message: {error}"));
                self.changed(cx);
                None
            }
        }
    }
    fn release_unsubmitted_claim(
        &mut self,
        bot: &str,
        message: &str,
        token: &str,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = locked(&self.store)
            .and_then(|store| store.release_unsubmitted_bot_claim(bot, message, token))
        {
            self.error = Some(format!(
                "Could not release Bot message claim: {error}. Inspect the conversation before retrying."
            ));
            self.changed(cx);
        }
    }
    /// A user message waiting behind a running turn steers that turn. A peer
    /// turn keeps its reply scoped to the peer's message, so it never mixes in
    /// the user's text.
    fn steer(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.inflight.get(id).is_some_and(|m| m.peer.is_some()) {
            return;
        }
        let Some(message) = self
            .roster
            .get(id)
            .and_then(|m| m.outbox.iter().find(|m| !self.held.contains(&m.id)))
            .filter(|m| self.persisted_messages.contains(&m.id))
            .cloned()
        else {
            return;
        };
        if let Some(view) = self.chats.get(id).cloned() {
            let Some(token) = self.claim_dispatch(id, &message, cx) else {
                return;
            };
            if view.update(cx, |v, cx| v.bot_steer(&message, cx)) {
                self.dispatch_claims.insert(message.id.clone());
            } else {
                self.release_unsubmitted_claim(id, &message.id, &token, cx);
            }
        }
    }
    fn steer_failed(&mut self, id: &str, message: &str, error: &str, cx: &mut Context<Self>) {
        if let Some(queued) = self
            .roster
            .get_mut(id)
            .and_then(|bot| bot.outbox.iter_mut().find(|m| m.id == message))
        {
            queued.error = Some(error.to_owned());
            self.save(cx);
            self.configure_chat(id, cx);
        }
    }
    fn chat_changed(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(view) = self.chats.get(id) else {
            return;
        };
        if self.active.as_deref() != Some(id)
            && self
                .roster
                .get(id)
                .is_some_and(|bot| bot.seen_rows < view.read(cx).bot_row_origin(cx))
        {
            view.update(cx, |view, cx| view.load(cx));
            return;
        }
        let view = view.read(cx);
        let row_count = view.bot_row_count(cx);
        let finished = view.bot_settled();
        let completion =
            (finished && self.inflight.contains_key(id)).then(|| NotificationFinished {
                summary: view.summary.clone(),
                body: view.notification_reply_body(cx),
                habit: false,
                was_cancelled: view.notification_was_cancelled(),
            });
        cx.emit(NotificationChanged);
        if let Some(completion) = completion {
            cx.emit(completion);
        }
        if self.active.as_deref() == Some(id)
            && let Some(bot) = self.roster.get_mut(id)
        {
            bot.seen_rows = row_count;
        }
        if finished {
            if let Some(mut sent) = self.inflight.remove(id) {
                let (mut ok, mut status) = self.chats[id].read(cx).external_outcome(cx);
                if sent.peer.is_some() && self.chats[id].read(cx).notification_was_cancelled() {
                    ok = false;
                    status = "Stopped before the peer reply completed. Inspect the conversation before retrying.".into();
                }
                if ok && let Err(error) = self.reply_to_peer(id, &sent, cx) {
                    ok = false;
                    status = format!(
                        "Peer reply could not be delivered: {error}. Inspect the conversation before retrying."
                    );
                }
                if ok {
                    if sent.peer.is_some() {
                        if let Some(bot) = self.roster.get_mut(id) {
                            bot.outbox.retain(|m| m.id != sent.id);
                        }
                        self.save(cx);
                        self.configure_chat(id, cx);
                    }
                } else if sent.peer.is_some() || !status.starts_with("Stopped") {
                    if let Some(bot) = self.roster.get_mut(id) {
                        if let Some(queued) = bot.outbox.iter_mut().find(|m| m.id == sent.id) {
                            queued.error = Some(status);
                        } else {
                            sent.id = bot::new_id();
                            sent.error = Some(status);
                            bot.outbox.insert(0, sent);
                        }
                    }
                    self.save(cx);
                    self.configure_chat(id, cx);
                }
            }
            self.check_completions(cx);
            self.reload_files(id, cx);
            self.dispatch(id, cx);
        }
        self.changed(cx);
    }
    pub(crate) fn open(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.prefs.bots || self.roster.get(id).is_none() {
            return;
        }
        if self.active.as_deref() != Some(id) {
            self.save_open_file(cx);
            self.pause_changes(cx);
            self.route = None;
            self.details.update(cx, |details, cx| details.reset(cx));
            self.fields
                .retain(|k, _| k.starts_with("file:") || k.starts_with("channel-"));
            self.files = None;
            self.memory_editor = None;
            self.memory_topics.clear();
            self.memory_loading = false;
            self.memory_load_error = None;
            self.settings = None;
        }
        self.close_classic_channels(cx);
        self.active = Some(id.into());
        self.ensure_chat(id, cx);
        self.reload_files(id, cx);
        cx.emit(TabOpened(id.into()));
        self.focus_active(window, cx);
        if let Some(view) = self.chats.get(id) {
            let count = view.read(cx).bot_row_count(cx);
            if let Some(bot) = self.roster.get_mut(id) {
                bot.seen_rows = count;
            }
        }
        self.save(cx);
    }
    pub(crate) fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_creation(window, cx) {
            return;
        }
        if self.settings.is_none()
            && let Some(chat) = self.chat()
        {
            window.focus(&chat.read(cx).input.focus_handle(cx), cx);
        } else {
            window.focus(&self.settings_focus, cx);
        }
    }
    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        self.close_classic_channels(cx);
        self.save_open_file(cx);
        self.pause_changes(cx);
        self.hide_html(cx);
        if self.active.take().is_some() {
            self.details.update(cx, |details, cx| details.reset(cx));
            self.settings = None;
            self.picker = None;
            self.save(cx);
        }
    }
    pub(crate) fn dismiss_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self
            .creation
            .as_ref()
            .is_some_and(|draft| draft.window.window_id() != window.window_handle().window_id())
        {
            return false;
        }
        if self.picker.take().is_some() || self.menu.take().is_some() {
            cx.notify();
            return true;
        }
        if self.creation.is_some() {
            self.cancel_creation(window, cx);
            return true;
        }
        if self.settings.take().is_none() {
            return false;
        }
        self.save_open_file(cx);
        if let Some(chat) = self.chat() {
            window.focus(&chat.read(cx).input.focus_handle(cx), cx);
        }
        cx.notify();
        true
    }
    fn reload_files(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.active.as_deref() != Some(id) {
            return;
        }
        self.file_load_revision += 1;
        let revision = self.file_load_revision;
        let Some(bot) = self.roster.get(id).cloned() else {
            return;
        };
        let root = self.root.clone();
        let id = id.to_string();
        let load_topics = self.settings == Some(SettingsSection::Memory);
        if load_topics {
            self.memory_loading = true;
            self.memory_load_error = None;
        }
        let work = cx.background_executor().spawn(async move {
            let files = bot::load_files(&root, &bot)?;
            let topics = load_topics.then(|| files::load_memory_topics(&root, &files));
            Ok::<_, String>((files, habits::load(&root, &bot.id)?, topics))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.active.as_deref() == Some(&id) && this.file_load_revision == revision {
                    match result {
                        Ok((files, habits, topics)) => {
                            this.files = Some(files);
                            this.habits = habits;
                            if let Some(topics) = topics {
                                this.publish_memory_topics(topics);
                            }
                        }
                        Err(error) => {
                            this.memory_load_error = Some(error.clone());
                            this.error = Some(error);
                        }
                    }
                    this.memory_loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(crate) fn rail(
        &mut self,
        project: Option<String>,
        liquid: &crate::liquid::Liquid,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.prefs.bots {
            return div().into_any_element();
        }
        let roster: Vec<_> = self
            .roster
            .bots
            .iter()
            .filter(|m| project.as_ref().is_none_or(|p| m.works_on(p)))
            .cloned()
            .collect();
        self.rail_ids = roster.iter().map(|m| m.id.clone()).collect();
        for bot in &roster {
            self.rail_bounds.entry(bot.id.clone()).or_default();
        }
        let has = !roster.is_empty();
        let add_project = project.clone();
        let heading = section_label("Bots").when(has, |d| {
            d.child(
                icon_button_hover("new-bot", "New bot", "plus", 0.)
                    .debug_selector(|| "sidebar-bot-button".into())
                    .size(px(20.))
                    .relative()
                    .child(liquid.row("add:bot", false))
                    .mr(px(-2.))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.create(add_project.clone().into_iter().collect(), w, cx)
                    })),
            )
        });
        div()
            .px_2()
            .pb_3()
            .child(heading)
            .when(!has, |d| {
                d.child(
                    div()
                        .id("new-bot-row")
                        .debug_selector(|| "new-bot".into())
                        .role(Role::Button)
                        .aria_label("New bot")
                        .h(px(32.))
                        .px_2()
                        .rounded_md()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(14.))
                        .text_color(content(0.5))
                        .relative()
                        .child(liquid.row("new-bot", false))
                        .hover(|s| s.text_color(content(1.)))
                        .child(icon("plus", 16.))
                        .child("New bot")
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.create(project.clone().into_iter().collect(), w, cx)
                        })),
                )
            })
            .children(roster.into_iter().map(|bot| {
                let active = self.active.as_deref() == Some(&bot.id);
                let state = self
                    .chats
                    .get(&bot.id)
                    .map(|v| v.read(cx).bot_status())
                    .unwrap_or("idle");
                let unread = self
                    .chats
                    .get(&bot.id)
                    .map(|v| {
                        v.read(cx)
                            .bot_rows(cx)
                            .iter()
                            .skip(bot.seen_rows.saturating_sub(v.read(cx).bot_row_origin(cx)))
                            .filter(|r| {
                                r.role == "assistant" && r.first && !r.text.trim().is_empty()
                            })
                            .count()
                    })
                    .unwrap_or(0);
                // Like BotRailSection, an open Bot has already been seen.
                // Activity belongs on the mascot, rather than an idle dot at
                // the end of every row.
                let unread = if active { 0 } else { unread };
                let label = if unread == 0 {
                    format!("{}, {state}", bot.label())
                } else {
                    format!(
                        "{}, {state}, {unread} new {}",
                        bot.label(),
                        if unread == 1 { "reply" } else { "replies" }
                    )
                };
                let id = bot.id.clone();
                let options = id.clone();
                let context = id.clone();
                let drag = id.clone();
                let slot = self.rail_bounds[&id].clone();
                div()
                    .id(SharedString::from(format!("bot-rail-{id}")))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("bot-rail-{id}")
                    })
                    .group("bot-rail-row")
                    .role(Role::Button)
                    .aria_label(label)
                    .h(px(32.))
                    .px_2()
                    .rounded_md()
                    .relative()
                    .flex()
                    .items_center()
                    .gap_2()
                    .opacity(if active { 1. } else { 0.65 })
                    .child(liquid.row(format!("bot:{id}"), active))
                    .hover(|s| s.opacity(1.))
                    .child(avatar::bot_avatar(&bot, state, 16., false))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(unread > 0, |d| d.pr(px(24.)))
                            .group_hover("bot-rail-row", |s| s.pr(px(24.)))
                            .text_size(px(14.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(bot.label()),
                    )
                    .when(unread > 0, |d| {
                        d.child(
                            div()
                                .absolute()
                                .right_1()
                                .top(px(4.))
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .group_hover("bot-rail-row", |s| s.invisible())
                                .child(
                                    div()
                                        .debug_selector({
                                            let id = bot.id.clone();
                                            move || format!("bot-rail-unread-{id}")
                                        })
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(accent_color()),
                                ),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .right_1()
                            .top(px(4.))
                            .invisible()
                            .group_hover("bot-rail-row", |s| s.visible())
                            .child(
                                icon_button("bot-options", "Bot options", "more-horizontal")
                                    .debug_selector({
                                        let id = bot.id.clone();
                                        move || format!("bot-rail-options-{id}")
                                    })
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        cx.stop_propagation();
                                        this.open_options(&options, None, w, cx);
                                    })),
                            ),
                    )
                    .child(slot.probe())
                    .on_click(cx.listener(move |this, _, w, cx| {
                        if std::mem::take(&mut this.skip_click) {
                            return;
                        }
                        this.open(&id, w, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, w, cx| {
                            this.open_options(&context, Some(event.position), w, cx)
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, _| {
                            this.skip_click = false;
                            this.drag = Some((drag.clone(), event.position, None));
                        }),
                    )
            }))
            // BotRailSection owns its menu and background dialog; the Bot
            // view paints them instead while it is open.
            .when(self.current().is_none() && !self.channels.open, |d| {
                d.children(self.options_surface(cx))
            })
            .into_any_element()
    }
}
impl gpui::Render for Bots {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.hide_html(cx);
        if self.channels.open {
            return self.channels_view(window, cx);
        }
        let Some(bot) = self.current().cloned() else {
            return div().into_any_element();
        };
        self.ensure_changes_summary(&bot, cx);
        let bg = self
            .backgrounds
            .entry(bot.id.clone())
            .or_insert_with(|| cx.new(|_| crate::project_background::ProjectBackground::default()))
            .clone();
        let empty = self.chat().is_none_or(|v| !v.read(cx).summary.has_messages);
        bg.update(cx, |bg, cx| {
            bg.configure(bot.background.clone(), empty, None, cx)
        });
        let header_visible = self.settings.is_none() && self.background.is_none();
        div()
            .size_full()
            .min_w_0()
            .flex()
            .relative()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape"
                    && this.settings.is_some()
                    && this.picker.is_none()
                    && this.menu.is_none()
                {
                    this.save_open_file(cx);
                    this.settings = None;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_any_mouse_down(|_, window, _| {
                crate::traffic_lights::focus_content(window);
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .relative()
                    .child(bg)
                    .children(
                        self.attention_views(&bot.id, cx)
                            .into_iter()
                            .map(|view| view.update(cx, |v, cx| v.bot_attention(window, cx))),
                    )
                    .when_some(self.chat(), |d, chat| {
                        chat.update(cx, |chat, cx| chat.show_bot_header(header_visible, cx));
                        d.child(div().flex_1().min_h_0().child(chat))
                    }),
            )
            .children(self.settings_modal(&bot, window, cx))
            .when(!self.creating(), |d| d.children(self.picker_surface(cx)))
            .children(self.options_surface(cx))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, _| {
                if let Some((_, start, over)) = &mut this.drag
                    && (event.position - *start).magnitude() > 5.
                {
                    this.skip_click = true;
                    *over = this.rail_bounds.iter().find_map(|(id, s)| {
                        s.get()
                            .filter(|b| b.contains(&event.position))
                            .map(|_| id.clone())
                    });
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.finish_reorder(cx);
                }),
            )
            .into_any_element()
    }
}
