//! Bot workspaces: one runtime per identity, shared by the rail, tabs,
//! overview and settings.
pub(crate) use crate::avatar;
use vorkspace_ui::radius;
pub(crate) mod cards;
mod creation;
mod document_pane;
mod effects;
mod files;
mod habit_panel;
#[cfg(test)]
mod header_tests;
mod host;
pub(crate) mod host_api;
mod html;
mod island;
pub(crate) use island::ISLAND_BREAKPOINT;
mod memory_ui;
mod notifications;
mod panel_stack;
mod panels;
mod peers;
mod roster;
pub(crate) use roster::{MembershipChanged, SharedRoster};
mod scheduler;
mod search;
#[cfg(test)]
mod tests;
mod work;

use crate::{
    conversation::{BotSubmitted, Conversation, SessionChanged},
    input::{self, Input},
    popover::{Anchor, AnchorSlot, Menu, MenuEvent, menu_popover, popover},
};
pub(crate) use document_pane::BotDocumentPane;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, Focusable, MouseButton, Role, SharedString,
    Subscription, WeakEntity, Window, div, prelude::*, px, rgb,
};
use panels::{OpenPicker, Route, SettingsSection};
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
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::theme::*;
pub(crate) fn conversation_in_other_window(
    open: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .justify_center()
        .items_center()
        .gap_2()
        .text_color(content(0.65))
        .child("This conversation is open in another window.")
        .child(
            div()
                .id("show-conversation-window")
                .role(Role::Button)
                .px_3()
                .py_1()
                .rounded(px(radius::SMALL))
                .bg(content(0.08))
                .hover(|button| button.bg(content(0.13)))
                .cursor_pointer()
                .on_click(open)
                .child("Show conversation"),
        )
}

fn load_backing_chat(
    db: &vorkspace_core::store::Store,
    root: &std::path::Path,
    bot: &Bot,
    session: i64,
) -> Result<roster::PreparedChat, String> {
    // Team work belongs exclusively to the Bot outbox. A migrated project
    // session may still have a legacy user follow-up queue.
    if bot.is_minion() {
        let mut queue = db.follow_ups(session)?;
        if !queue.paused {
            queue.paused = true;
            queue.revision += 1;
            db.accept_follow_ups(session, &queue)?;
        }
    }
    let mut summary = db.bot_session(session)?
        .ok_or("This Bot's saved conversation is unavailable. Reset its conversation explicitly to start a new one.")?;
    let settings = db.agent_settings(session)?.unwrap_or_default();
    let mut warning = None;
    if !bot.is_minion() && settings.worktree_cwd.is_none() {
        let home = bot::home::ensure(root, bot, Some(std::path::Path::new(&summary.cwd)))?;
        db.move_bot_home(&summary, &home.to_string_lossy())?;
        summary.cwd = home.to_string_lossy().into_owned();
        warning = bot::home::write_session(root, bot, &summary, &settings.account).err();
    }
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
        .collect();
    Ok((summary, None, delivered, warning))
}

pub(crate) struct Changed;
pub(crate) struct TabOpened(pub String);
#[derive(Clone, Debug)]
pub(crate) enum OpenWorkspacePane {
    Session { bot: String, session: i64 },
    Document { bot: String, document: String },
}
#[derive(Clone, Debug)]
pub(crate) struct BotDocumentSource {
    pub bot: String,
    pub document: bot::Document,
    pub desktop: bool,
}
pub(crate) use notifications::{NotificationChanged, NotificationFinished};
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
pub(crate) use crate::conversation::{
    BotConversations, MinionRegistry, TeamHistories, TeamHistory,
};

pub(crate) struct Bots {
    minion_effects: effects::MinionEffects,
    pub(crate) minion_registry: MinionRegistry,
    pub(crate) team_histories: TeamHistories,
    pub(crate) conversation_views: BotConversations,
    retired_teams: HashSet<String>,
    parent_window: Option<gpui::AnyWindowHandle>,
    owner: std::rc::Rc<dyn host_api::BotsHost>,
    store: Database,
    root: PathBuf,
    prefs: vorkspace_ui::preferences::Preferences,
    pub(crate) roster: Roster,
    pub(crate) roster_loaded: bool,
    shared_roster: Entity<SharedRoster>,
    roster_revision: u64,
    pending_roster_writes: usize,
    vorkspace: Option<String>,
    /// Bots placed in another vorkspace, hidden from this window's rail.
    /// A Bot created a moment ago is not here, so it shows at once.
    pub(crate) vorkspace_hidden: HashSet<String>,
    pub(crate) active: Option<String>,
    chats: HashMap<String, Entity<Conversation>>,
    bridge: Option<bot::bridge::Server>,
    observations: Vec<Subscription>,
    _bridge_task: Option<gpui::Task<()>>,
    _clock: Option<gpui::Task<()>>,
    loading_chats: HashSet<String>,
    awaiting_creation: HashSet<String>,
    work: work::Cache,
    pub(crate) error: Option<String>,
    notice_region: AnchorSlot,
    habit_panel: Entity<habit_panel::HabitPanel>,
    open_islands: HashSet<String>,
    expanded_islands: HashMap<String, island::Section>,
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
}
impl EventEmitter<Changed> for Bots {}
impl EventEmitter<TabOpened> for Bots {}
impl EventEmitter<OpenWorkspacePane> for Bots {}
impl Bots {
    pub(crate) fn presentation_bot(&self, id: &str) -> Option<Bot> {
        let mut actor = self.roster.effective_bot(id)?;
        if let Some(parent) = self.roster.parent_of(id) {
            actor.color = parent.color;
            actor.fg_color = parent.fg_color;
        }
        Some(actor)
    }
    fn sync_team_histories(&self, cx: &mut Context<Self>) {
        let mut histories = HashMap::new();
        for actor in &self.roster.bots {
            if actor.is_minion()
                && let Some(session) = actor.session_id
            {
                histories.insert(
                    session,
                    TeamHistory {
                        actor: actor.clone(),
                        parent: self.roster.parent_of(&actor.id).cloned(),
                    },
                );
            }
        }
        *self.team_histories.borrow_mut() = histories;
        for view in self.chats.values() {
            self.conversation_views
                .borrow_mut()
                .insert(view.read(cx).summary.id, view.downgrade());
            view.update(cx, |view, cx| {
                view.team_histories = Some(self.team_histories.clone());
                view.refresh_team_history(cx);
            });
        }
    }
    pub(crate) fn minion_parent(&self, id: &str) -> Option<String> {
        self.roster.parent_of(id).map(|b| b.id.clone())
    }
    pub(crate) fn team_members(&self, parent: &str) -> Vec<Bot> {
        self.roster
            .team(parent)
            .filter(|b| b.is_minion())
            .filter_map(|b| self.presentation_bot(&b.id))
            .collect()
    }
    pub(crate) fn minion_read_only(&self, id: &str) -> bool {
        self.roster.get(id).is_some_and(Bot::is_minion)
    }
    #[cfg(test)]
    fn session_user_read_only(&self, session: i64) -> bool {
        self.roster.of_session(session).is_some_and(Bot::is_minion)
            || self.team_histories.borrow().contains_key(&session)
    }
    pub(crate) fn conversation_for_session(
        &self,
        id: i64,
        cx: &App,
    ) -> Option<Entity<Conversation>> {
        self.chats
            .values()
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
        if let Some(view) = self
            .chats
            .values()
            .chain(self.running.values().map(|r| &r.view))
            .find(|v| v.read(cx).summary.id == id)
        {
            view.update(cx, |v, cx| v.stop_if_busy(cx));
        }
    }
    pub(crate) fn bind_window(&mut self, window: &Window) {
        self.parent_window = Some(window.window_handle());
    }
    pub(crate) fn open_session_workspace_pane(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(bot) = self.active.clone() else {
            return;
        };
        self.open_islands.remove(&bot);
        cx.emit(OpenWorkspacePane::Session { bot, session: id });
        cx.notify();
    }

    pub(crate) fn open_document_workspace_pane(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(bot) = self.current() else {
            return;
        };
        if !bot.documents.iter().any(|document| document.id == id) {
            return;
        }
        let bot = bot.id.clone();
        self.open_islands.remove(&bot);
        cx.emit(OpenWorkspacePane::Document {
            bot,
            document: id.to_owned(),
        });
        cx.notify();
    }

    pub(crate) fn document_for_pane(&self, bot: &str, id: &str) -> Option<BotDocumentSource> {
        let actor = self.roster.get(bot)?;
        let listed = actor.documents.iter().find(|document| document.id == id)?;
        let desktop = actor.desktop_document_ids.iter().any(|source| source == id);
        Some(BotDocumentSource {
            bot: bot.into(),
            // Resolve through this Bot's roster so a document ID alone never
            // grants access to another Bot's output.
            document: listed.clone(),
            desktop,
        })
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
    pub(crate) fn visible_idle_sessions(&self, visible: &HashSet<String>) -> Vec<i64> {
        visible
            .iter()
            .filter_map(|id| self.roster.get(id).and_then(|bot| bot.session_id))
            .collect()
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
        shared_roster: Entity<SharedRoster>,
        owner: std::rc::Rc<dyn host_api::BotsHost>,
        prefs: vorkspace_ui::preferences::Preferences,
        active: Option<String>,
        vorkspace: Option<String>,
        notice_region: AnchorSlot,
        cx: &mut Context<Self>,
    ) -> Self {
        bot::set_minions_enabled(prefs.bots && prefs.minions);
        let shared = shared_roster.read(cx);
        let store = shared.store.clone();
        let root = shared.root.clone();
        let roster_revision = shared.revision;
        let (roster, roster_error) = match &shared.snapshot {
            Ok(roster) => (roster.fork(), None),
            Err(error) => (Roster::default(), Some(error.clone())),
        };
        let roster_subscription =
            cx.observe(&shared_roster, |this, _, cx| this.sync_shared_roster(cx));
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
        let manager = cx.entity();
        let habit_panel = cx.new(|cx| habit_panel::HabitPanel::new(manager, cx));
        let mut this = Self {
            parent_window: None,
            owner,
            store,
            root,
            prefs,
            roster,
            roster_loaded: roster_error.is_none(),
            shared_roster,
            roster_revision,
            pending_roster_writes: 0,
            vorkspace,
            vorkspace_hidden: HashSet::new(),
            active: None,
            chats: HashMap::new(),
            bridge,
            observations: vec![roster_subscription],
            _bridge_task: None,
            _clock: None,
            loading_chats: HashSet::new(),
            awaiting_creation: HashSet::new(),
            work: work::Cache::default(),
            error: roster_error,
            notice_region,
            habit_panel,
            open_islands: HashSet::new(),
            expanded_islands: HashMap::new(),
            minion_registry: Default::default(),
            team_histories: Default::default(),
            minion_effects: Default::default(),
            conversation_views: Default::default(),
            retired_teams: Default::default(),
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
        this.sync_team_histories(cx);
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
        let _ = cx;
        self.is_bot_session(id) || self.running.contains_key(&id)
    }
    pub(crate) fn app_project_allowed(&self, caller: i64, project: &str) -> bool {
        self.roster
            .of_session(caller)
            .or_else(|| {
                self.running
                    .get(&caller)
                    .and_then(|run| self.roster.get(&run.bot))
            })
            .is_some_and(|actor| {
                self.roster.is_active(&actor.id)
                    && (!actor.is_minion() || (self.prefs.bots && self.prefs.minions))
                    && self
                        .roster
                        .storage_owner(&actor.id)
                        .is_some_and(|owner| owner.works_on(project))
            })
    }
    #[cfg(test)]
    pub(crate) fn hidden_session(&self, id: i64) -> bool {
        self.is_bot_session(id)
            || self.roster.bots.iter().any(|m| {
                !m.show_started_sessions_in_sidebar && m.launches.iter().any(|l| l.session_id == id)
            })
    }
    pub(crate) fn set_preferences(
        &mut self,
        prefs: vorkspace_ui::preferences::Preferences,
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
        cx.notify();
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
    pub(crate) fn set_visible_panes(&mut self, visible: &HashSet<String>, cx: &mut Context<Self>) {
        for id in visible {
            self.ensure_chat(id, cx);
        }
        for (id, chat) in &self.chats {
            chat.update(cx, |chat, cx| {
                chat.transcript
                    .update(cx, |view, cx| view.visible(visible.contains(id), cx));
            });
        }
    }
    /// Queue an application message through the Bot's ordinary outbox.
    pub(crate) fn send_message(
        &mut self,
        id: &str,
        text: String,
        attachments: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.roster.get(id).is_some_and(Bot::is_minion) {
            return Err("Users cannot message Minions; delegate through their parent Bot".into());
        }
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
        let actors = self
            .roster
            .bots
            .iter()
            .filter_map(|b| self.presentation_bot(&b.id))
            .collect::<Vec<_>>();
        let looks = crate::transcript::peer_looks(&actors);
        for view in self.chats.values() {
            let transcript = view.read(cx).transcript.clone();
            transcript.update(cx, |t, cx| t.set_peer_looks(looks.clone(), cx));
        }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.sync_team_histories(cx);
        self.sync_peer_looks(cx);
        cx.emit(Changed);
        cx.notify();
    }
    pub(crate) fn set_vorkspace(&mut self, id: String, hidden: HashSet<String>) {
        self.vorkspace = Some(id);
        self.vorkspace_hidden = hidden;
    }

    pub(crate) fn confirmed_missing(&self, id: &str, cx: &App) -> bool {
        let shared = self.shared_roster.read(cx);
        self.roster_loaded
            && !self.awaiting_creation.contains(id)
            && (self.roster.get(id).is_none() || shared.removed.contains_key(id))
            && shared
                .committed()
                .is_some_and(|roster| roster.get(id).is_none())
    }

    fn persist_roster<T: Send + 'static>(
        &mut self,
        finalize: impl FnOnce(&vorkspace_core::store::Store) -> Result<T, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<Result<T, String>> {
        self.pending_roster_writes += 1;
        self.shared_roster.update(cx, |shared, cx| {
            shared.save(self.roster.clone(), self.vorkspace.clone(), finalize, cx)
        })
    }

    fn roster_write_finished(&mut self, cx: &mut Context<Self>) {
        self.pending_roster_writes -= 1;
        self.sync_shared_roster(cx);
    }

    fn sync_shared_roster(&mut self, cx: &mut Context<Self>) {
        let shared = self.shared_roster.read(cx);
        // A queued snapshot still needs its original editor's save baseline.
        if self.pending_roster_writes > 0 || self.roster_revision == shared.revision {
            return;
        }
        let Ok(committed) = &shared.snapshot else {
            return;
        };
        let deleted: HashSet<_> = self
            .chats
            .keys()
            .chain(self.running.values().map(|run| &run.bot))
            .filter(|id| shared.removed.contains_key(*id))
            .cloned()
            .collect();
        let changed = match self.roster.rebase_onto(committed) {
            Ok(changed) => changed,
            Err(error) => {
                for id in deleted {
                    self.stop_deleted_bot(&id, cx);
                }
                self.error = Some(error);
                cx.emit(Changed);
                cx.notify();
                return;
            }
        };
        let deletion_published = shared
            .removed
            .values()
            .any(|&revision| revision > self.roster_revision);
        self.roster_revision = shared.revision;
        self.roster_loaded = true;
        self.held.clone_from(&self.roster.held_messages);
        self.persisted_messages = self
            .roster
            .bots
            .iter()
            .flat_map(|bot| {
                let saved = committed.get(&bot.id);
                bot.outbox
                    .iter()
                    .filter(move |message| saved.is_some_and(|bot| bot.outbox.contains(message)))
                    .map(|message| message.id.clone())
            })
            .collect();
        for id in deleted {
            self.stop_deleted_bot(&id, cx);
        }
        if changed || deletion_published {
            if let Some(id) = self.active.clone() {
                if self.roster.get(&id).is_some() {
                    self.configure_chat(&id, cx);
                } else {
                    self.close(cx);
                }
            }
            let queued = self
                .chats
                .keys()
                .filter(|id| {
                    self.roster.get(id).is_some_and(|bot| {
                        !bot.outbox.is_empty() || !bot.pending_reports.is_empty()
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            for id in queued {
                self.configure_chat(&id, cx);
                self.dispatch(&id, cx);
            }
            self.changed(cx);
        }
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
        let queued = self
            .roster
            .bots
            .iter()
            .map(|m| (m.id.clone(), m.outbox.clone()))
            .collect::<Vec<_>>();
        let saved = self.persist_roster(|_| Ok(()), cx);
        cx.spawn(async move |this, cx| {
            let result = saved
                .recv()
                .await
                .unwrap_or_else(|_| Err("Bot roster write was interrupted".into()));
            let _ = this.update(cx, |this, cx| {
                match result {
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
                }
                this.roster_write_finished(cx);
            });
        })
        .detach();
        self.changed(cx);
    }
    fn release_chat(&self, id: &str, cx: &mut Context<Self>) {
        let owner = cx.weak_entity();
        self.shared_roster
            .update(cx, |shared, _| shared.release_chat(id, &owner));
    }

    fn ensure_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.awaiting_creation.contains(id) {
            return;
        }
        if self.chats.contains_key(id) || self.loading_chats.contains(id) {
            return;
        }
        let Some(bot) = self.presentation_bot(id) else {
            return;
        };
        let owner = cx.weak_entity();
        if !self
            .shared_roster
            .update(cx, |shared, _| shared.claim_chat(id, owner))
        {
            return;
        }
        self.loading_chats.insert(id.into());
        let id = id.to_owned();
        let prepared = if bot.session_id.is_none() && bot.unavailable_session.is_none() {
            let prefs = self.prefs.clone();
            let id = id.clone();
            let reply = self
                .shared_roster
                .update(cx, |shared, cx| shared.prepare_chat(id, prefs, cx));
            cx.background_executor().spawn(async move {
                reply
                    .recv()
                    .await
                    .unwrap_or_else(|_| Err("Bot conversation preparation was interrupted".into()))
            })
        } else {
            let store = self.store.clone();
            let root = self.root.clone();
            cx.background_executor().spawn(async move {
                if let Some(missing) = &bot.unavailable_session {
                    return Err(format!("This Bot's saved conversation ({missing}) is unavailable. Its identity and history have been preserved."));
                }
                let db = locked(&store)?;
                load_backing_chat(&db, &root, &bot, bot.session_id.expect("existing Bot session"))
            })
        };
        cx.spawn(async move |this, cx| {
            let result = prepared.await;
            let _ = this.update(cx, |this, cx| {
                this.loading_chats.remove(&id);
                let (summary, settings, delivered, warning) = match result {
                    Ok(value) => value,
                    Err(e) => {
                        this.release_chat(&id, cx);
                        this.error = Some(e);
                        this.changed(cx);
                        return;
                    }
                };
                if let Some(warning) = warning {
                    this.error = Some(warning);
                }
                if !this.roster.hidden_sessions.contains(&summary.id) {
                    this.roster.hidden_sessions.push(summary.id);
                }
                let Some(bot) = this.roster.get_mut(&id) else {
                    this.release_chat(&id, cx);
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
                let Some(bot) = this.presentation_bot(&id) else { return; };
                let page_chat = this.active.as_deref() == Some(id.as_str());
                let existing = this.owner.session_entity(summary.id, cx)
                    .filter(|view| bot.is_minion() && view.read(cx).settings.bot.is_none());
                let view = existing.unwrap_or_else(|| cx.new(|cx| Conversation::new(summary, this.store.clone(), cx)));
                this.conversation_views.borrow_mut().insert(view.read(cx).summary.id, view.downgrade());
                this.sync_team_histories(cx);
                let root = this.root.clone();
                let env = this
                    .bridge
                    .as_ref()
                    .map(|b| b.environment(view.read(cx).summary.id))
                    .unwrap_or_default();
                view.update(cx, |view, cx| {
                    view.team_histories = Some(this.team_histories.clone());
                    view.allow_storage_paging = page_chat;
                    view.configure_bot(bot, root, env, cx);
                    view.refresh_team_history(cx);
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
                let mut status = view.read(cx).bot_status_label();
                this.observations
                    .push(cx.observe(&view, move |this, view, cx| {
                        let next = view.read(cx).bot_status_label();
                        if this.active.as_ref() == Some(&source) || status != next {
                            cx.notify();
                        }
                        status = next;
                        this.dispatch(&source, cx);
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
                    move |this, _, event: &crate::transcript::bot_cards::CardAction, cx| {
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
            });
        })
        .detach();
    }
    /// Retry clears a queued message's error; discard drops it.
    fn queue_changed(&mut self, source: &str, id: &str, remove: bool, cx: &mut Context<Self>) {
        if self.roster.get(source).is_some_and(Bot::is_minion) {
            return;
        }
        self.recover_queue(source, id, remove, cx);
    }
    /// The Bot host validates parent ownership before using this path for a
    /// Minion. User transcript controls continue through queue_changed above.
    fn recover_queue(&mut self, source: &str, id: &str, remove: bool, cx: &mut Context<Self>) {
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
        self.sync_team_histories(cx);
        self.refresh_work(id, cx);
        if let (Some(bot), Some(view)) = (self.presentation_bot(id), self.chats.get(id).cloned()) {
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
        if self.roster.get(id).is_some_and(Bot::is_minion) {
            self.error =
                Some("Users cannot message Minions; delegate through their parent Bot".into());
            if let (Some(draft), Some(view)) = (draft_id.as_deref(), self.chats.get(id)) {
                view.update(cx, |v, cx| {
                    v.bot_draft_outbox_failed(draft, "Users cannot message Minions", cx)
                });
            }
            return;
        }
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
        if !self.roster.is_active(id)
            || (self.roster.get(id).is_some_and(Bot::is_minion)
                && !(self.prefs.bots && self.prefs.minions))
        {
            return;
        }
        if self
            .roster
            .get(id)
            .and_then(|b| b.outbox.first())
            .is_some_and(|m| {
                m.peer
                    .as_ref()
                    .is_some_and(|p| !self.roster.can_communicate(&p.from, id))
                    || (self.roster.get(id).is_some_and(Bot::is_minion) && m.peer.is_none())
            })
        {
            return;
        }
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
        // Refreshing context notifies the view. A held/restoring recipient must
        // not enter a configure → observe → dispatch loop before admission.
        if !view.read(cx).bot_ready() {
            if view.read(cx).needs_history_restore() {
                view.update(cx, |view, cx| view.load(cx));
            }
            return;
        }
        let Some(token) = self.claim_dispatch(id, &message, cx) else {
            return;
        };
        let mode = self.roster.message_modes.get(&message.id).cloned();
        self.configure_chat(id, cx);
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
        if !self.roster.is_active(id) || self.roster.get(id).is_some_and(Bot::is_minion) {
            return;
        }
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
        if self.focus_chat_window(id, cx) {
            return;
        }
        if self.open_workspace_pane(id, window, cx) {
            cx.emit(TabOpened(id.into()));
        }
    }

    fn other_chat_owner(&self, id: &str, cx: &Context<Self>) -> Option<WeakEntity<Self>> {
        self.shared_roster
            .read(cx)
            .chat_owner(id)
            .filter(|owner| **owner != cx.weak_entity())
            .cloned()
    }

    pub(crate) fn focus_chat_window(&self, id: &str, cx: &mut Context<Self>) -> bool {
        let Some(owner) = self.other_chat_owner(id, cx) else {
            return false;
        };
        let id = id.to_owned();
        cx.defer(move |cx| {
            let Some(manager) = owner.upgrade() else {
                return;
            };
            let manager = manager.read(cx);
            let Some(handle) = manager.parent_window else {
                return;
            };
            let host = manager.owner.clone();
            let _ = handle.update(cx, |_, window, cx| {
                host.open_owned_bot(&id, window, cx);
                vorkspace_ui::traffic_lights::show(window);
                window.activate_window();
            });
        });
        true
    }
    /// Prepare an already selected Bot pane synchronously. Sidebar navigation
    /// still uses `open` to ask Shell to select its workspace tab.
    pub(crate) fn open_workspace_pane(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.prefs.bots || self.roster.get(id).is_none() {
            return false;
        }
        let already_active = self.active.as_deref() == Some(id);
        self.set_workspace_context(Some(id), cx);
        if already_active {
            self.ensure_chat(id, cx);
            self.reload_files(id, cx);
            if let Some(view) = self.chats.get(id) {
                let count = view.read(cx).bot_row_count(cx);
                if let Some(bot) = self.roster.get_mut(id) {
                    bot.seen_rows = count;
                }
            }
            self.save(cx);
        }
        self.focus_active(window, cx);
        true
    }
    /// Follow the Bot that owns the visible workspace context without
    /// changing the selected pane or claiming keyboard focus from a sibling.
    pub(crate) fn set_workspace_context(&mut self, bot: Option<&str>, cx: &mut Context<Self>) {
        let bot = bot
            .filter(|id| self.prefs.bots && self.roster.get(id).is_some())
            .map(str::to_owned);
        if self.active == bot {
            return;
        }
        let Some(id) = bot else {
            self.park_workspace_context(cx);
            return;
        };
        if self.roster.get(&id).is_some_and(Bot::is_retired)
            && let Some(parent) = self.minion_parent(&id)
        {
            self.retired_teams.insert(parent);
        }
        self.save_open_file(cx);
        self.route = None;
        self.habit_panel.update(cx, |panel, cx| panel.reset(cx));
        self.fields.retain(|k, _| k.starts_with("file:"));
        self.files = None;
        self.memory_editor = None;
        self.memory_topics.clear();
        self.memory_loading = false;
        self.memory_load_error = None;
        self.settings = None;
        self.active = Some(id.clone());
        self.ensure_chat(&id, cx);
        self.reload_files(&id, cx);
        if let Some(view) = self.chats.get(&id) {
            let count = view.read(cx).bot_row_count(cx);
            if let Some(bot) = self.roster.get_mut(&id) {
                bot.seen_rows = count;
            }
        }
        self.save(cx);
        cx.notify();
    }
    pub(crate) fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_creation(window, cx) {
            return;
        }
        if self.settings.is_none()
            && let Some(chat) = self.chat()
        {
            if chat.read(cx).user_read_only() {
                window.focus(&self.settings_focus, cx);
            } else {
                window.focus(&chat.read(cx).input.focus_handle(cx), cx);
            }
        } else {
            window.focus(&self.settings_focus, cx);
        }
    }
    /// Focus this Bot's chat or modal.
    pub(crate) fn focus_bot_pane(&self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.active.as_deref() == Some(id) {
            self.focus_active(window, cx);
        }
    }
    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        self.park_workspace_context(cx);
    }
    fn park_workspace_context(&mut self, cx: &mut Context<Self>) {
        self.save_open_file(cx);
        if self.active.take().is_some() {
            self.habit_panel.update(cx, |panel, cx| panel.reset(cx));
            self.settings = None;
            self.picker = None;
            self.save(cx);
        }
        cx.notify();
    }
    /// Opens the current Bot's look editor in its settings, for captures.
    #[cfg(test)]
    pub(crate) fn open_look_for_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active.clone() {
            self.settings = Some(SettingsSection::General);
            self.open_settings_look(&id, window, cx);
        }
    }
    /// Shows one settings section, for captures of each page.
    #[cfg(test)]
    pub(crate) fn show_settings_section(&mut self, section: &str, cx: &mut Context<Self>) {
        self.settings = Some(match section {
            "soul" => SettingsSection::Soul,
            "memory" => SettingsSection::Memory,
            "habits" => SettingsSection::Habits,
            _ => SettingsSection::General,
        });
        cx.notify();
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
    pub(crate) fn rail(&mut self, project: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        if !self.prefs.bots {
            return div().into_any_element();
        }
        let roster: Vec<_> = self
            .roster
            .bots
            .iter()
            .filter(|m| !m.is_minion())
            .filter(|m| project.as_ref().is_none_or(|p| m.works_on(p)))
            .filter(|m| !self.vorkspace_hidden.contains(&m.id))
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
                icon_button("new-bot", "New bot", "plus")
                    .debug_selector(|| "sidebar-bot-button".into())
                    .size(px(20.))
                    .relative()
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
                    vorkspace_ui::theme::keyboard_reachable(div().id("new-bot-row"), true)
                        .debug_selector(|| "new-bot".into())
                        .role(Role::Button)
                        .aria_label("New bot")
                        .h(px(32.))
                        .px_2()
                        .rounded(px(radius::CONTROL))
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(14.))
                        .text_color(content(0.5))
                        .relative()
                        .hover(|s| {
                            s.bg(vorkspace_ui::fluid::hover(0.08))
                                .text_color(content(1.))
                        })
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
                let team = self.team_rail(&bot, cx);
                let row = div()
                    .id(SharedString::from(format!("bot-rail-{id}")))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("bot-rail-{id}")
                    })
                    .group("bot-rail-row")
                    .role(Role::Button)
                    .aria_label(label)
                    .aria_selected(active)
                    .h(px(32.))
                    .px_2()
                    .rounded(px(radius::CONTROL))
                    .relative()
                    .flex()
                    .items_center()
                    .gap_2()
                    .opacity(if active { 1. } else { 0.65 })
                    .when(active, |d| d.bg(content(0.1)))
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)).opacity(1.))
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
                    );
                div().child(row).child(team)
            }))
            // BotRailSection owns its menu and background dialog; the Bot
            // view paints them instead while it is open.
            .when(self.current().is_none(), |d| {
                d.children(self.options_surface(cx))
            })
            .into_any_element()
    }
}
impl Bots {
    pub(crate) fn render_bot_pane(
        &mut self,
        id: &str,
        width: f32,
        linked: Option<Entity<crate::notifications::linked_activity::LinkedActivity>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(bot) = self.presentation_bot(id) else {
            return div().into_any_element();
        };
        if self.other_chat_owner(id, cx).is_some() {
            let id = id.to_owned();
            return conversation_in_other_window(cx.listener(
                move |manager, _: &gpui::ClickEvent, window, cx| manager.open(&id, window, cx),
            ))
            .into_any_element();
        }
        let bg = self
            .backgrounds
            .entry(bot.id.clone())
            .or_insert_with(|| cx.new(|_| crate::project_background::ProjectBackground::default()))
            .clone();
        let chat = self.chat_for_action(id, cx);
        let empty = chat
            .as_ref()
            .is_none_or(|v| !v.read(cx).summary.has_messages);
        // `active` owns the one settings/modal surface. Shell owns keyboard
        // focus, which may be on a session or document beside this Bot.
        let owns_overlays = self.active.as_deref() == Some(id);
        bg.update(cx, |bg, cx| {
            bg.configure(bot.background.clone(), empty, None, cx)
        });
        let wide = width >= island::ISLAND_BREAKPOINT;
        let show_island = !bot.is_minion() && !self.creating();
        let overlay = show_island && !wide && self.open_islands.contains(id);
        let escape_id = id.to_string();
        let header_action: Option<crate::bot_header::Activate> =
            (show_island && !wide).then(|| {
                let manager = cx.weak_entity();
                let id = id.to_string();
                std::rc::Rc::new(move |_: &mut Window, cx: &mut App| {
                    let _ = manager.update(cx, |manager, cx| manager.toggle_island(&id, cx));
                }) as crate::bot_header::Activate
            });
        let header_visible =
            !owns_overlays || (self.settings.is_none() && self.background.is_none());
        div()
            .size_full()
            .min_w_0()
            .flex()
            .relative()
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape"
                    && overlay
                    && this.open_islands.remove(&escape_id)
                {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
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
                vorkspace_ui::traffic_lights::focus_content(window);
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
                    .when_some(chat, |d, chat| {
                        chat.update(cx, |chat, cx| {
                            chat.show_bot_header(header_visible, cx);
                            chat.set_bot_header_action(header_action);
                        });
                        d.child(div().flex_1().min_h_0().child(chat))
                    })
                    .children(linked.map(|notice| {
                        div()
                            .absolute()
                            .top_3()
                            .right(px(if show_island && !wide { 52. } else { 12. }))
                            .w(px(320.))
                            .max_w_full()
                            .child(notice)
                    })),
            )
            .when(show_island && wide, |d| d.child(self.island_lane(&bot, cx)))
            .when(show_island && !wide && !overlay, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_3()
                        .right_3()
                        .child(self.island_trigger(&bot, cx)),
                )
            })
            .when(overlay, |d| d.child(self.island_overlay(&bot, width, cx)))
            .when(owns_overlays, |d| {
                d.children(self.settings_modal(&bot, window, cx))
            })
            .when(owns_overlays && !self.creating() && !bot.is_minion(), |d| {
                d.children(self.picker_surface(cx))
            })
            .when(owns_overlays && !bot.is_minion(), |d| {
                d.children(self.options_surface(cx))
            })
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

impl gpui::Render for Bots {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(id) = self.active.clone() else {
            return div().into_any_element();
        };
        self.render_bot_pane(&id, window.viewport_size().width.as_f32(), None, window, cx)
    }
}
