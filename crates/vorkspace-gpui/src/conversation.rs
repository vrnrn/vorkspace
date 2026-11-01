mod accounts;
mod approvals;
mod attachment_drop;
mod bot;
pub(crate) mod bot_settings;
mod btw;
mod commands;
mod composer_draft;
pub(crate) mod composer_features;
mod composer_shelf;
mod composer_view;
mod edit_last_turn;
mod follow_up;
mod git_context;
mod history;
mod local_submissions;
mod model_menu;
mod notifications;
#[cfg(test)]
pub(crate) mod receive_tests;
mod remote;
pub(crate) mod remote_plans;
mod response_menu;
mod response_transfer;
mod responsive_toolbar;
mod session_load;
#[cfg(test)]
mod submission_tests;
mod team_history;
mod ticket_context;
mod title;
mod turn_flow;
use crate::{
    git_tools::git_picker::{GitPicker, Mode as GitMode, Picked as GitPicked},
    input,
    pickers::{
        ModelList, ModelPicked, cached_model_name, model_display_name, model_name, remember_model,
    },
    popover::{
        Align, Anchor, AnchorSlot, Flyout, Menu, MenuAction, MenuEvent, MenuItem, MenuStyle, Side,
        menu_popover, popover,
    },
    response_actions::{Request as ResponseRequest, Transfer},
    transcript::TranscriptView,
};
pub(crate) use accounts::UsageResumed;
pub(crate) use bot::QueueChanged as BotQueueChanged;
pub(crate) use bot::SteerFailed as BotSteerFailed;
pub(crate) use bot::Submitted as BotSubmitted;
use gpui::{
    AnyElement, Context, CursorStyle, Entity, EventEmitter, Focusable, MouseButton, Role,
    SharedString, StyleRefinement, Subscription, Window, canvas, div, prelude::*, px,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use vorkspace_core::{
    agent::{AccessMode, Approval, Control, Decision, Settings},
    catalog::{self, Model},
};
use vorkspace_core::{provider::Kind, store::Session};
use vorkspace_runtime::runtime::{Database, locked, run_turn};
use vorkspace_ui::clock::Instant;
use vorkspace_ui::{composer_motion::ComposerMotion, theme::*};
// Keep empty and docked composers aligned with the transcript column. Below
// this width the empty session drops its title.
const COMPOSER_COLUMN: f32 = COLUMN;
const UNCONFIRMED_TURN: &str =
    "Provider connection ended before completion was confirmed. This turn was not replayed.";
// RUNTIME_MODE_LABEL and RUNTIME_MODE_HINT from the React session model,
// with AccessPicker's icons; Full access is amber.
const ACCESS_MODES: &[(&str, &str, &str, &str)] = &[
    (
        "supervised",
        "Supervised",
        "Ask before commands and file changes.",
        "square-lock",
    ),
    (
        "auto-accept-edits",
        "Auto-accept edits",
        "Auto-approve edits, ask before other actions.",
        "pencil-edit",
    ),
    (
        "auto",
        "Auto",
        "An AI reviewer can approve or deny actions.",
        "sparkles",
    ),
    (
        "full-access",
        "Full access",
        "Allow commands, edits, and supported MCP confirmations in non-plan turns without prompts.",
        "shield-alert",
    ),
];
// Tailwind v4 accents the composer menus tint their icons with.
const YELLOW_300: u32 = 0xffdf20;
const SKY_300: u32 = 0x74d4ff;
const FUCHSIA_300: u32 = 0xf4a8ff;
fn tint(color: u32, alpha: f32) -> gpui::Hsla {
    let mut c: gpui::Hsla = gpui::rgb(color).into();
    c.a = alpha;
    c
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ComposerMenu {
    Add,
    Model,
    /// ModelPicker's recent models, on right-click or `App: Switch Model`.
    Recent,
    Access,
    Account,
    Workspace,
    CompactSettings,
    /// A Bot's model setting pill: 0 is effort, then each model control.
    Setting(usize),
}
struct OpenMenu {
    kind: ComposerMenu,
    menu: Entity<Menu>,
    _events: Vec<Subscription>,
}
use response_menu::ResponseMenu;
pub(crate) struct SessionChanged;
/// A tool label changed; token/output changes do not wake session discovery.
pub(crate) struct SessionActivityChanged;
pub(crate) struct ChooseFolder(pub(crate) AnchorSlot);
/// The review card asked to open the session's recorded changes.
pub(crate) struct OpenSessionChanges(pub(crate) crate::git_tools::review::ReviewTab);
pub(crate) struct Conversation {
    editing_last_turn: edit_last_turn::State,
    btw: Option<Entity<btw::Btw>>,
    pub(crate) summary: Session,
    remote_state: remote::State,
    remote_plans: remote_plans::State,
    store: Database,
    pub(crate) transcript: Entity<TranscriptView>,
    pub(crate) review: Entity<crate::git_tools::session_review::SessionReview>,
    _review_events: Subscription,
    _review_observation: Subscription,
    pub(crate) input: Entity<input::Input>,
    completion: Entity<crate::composer::composer_completion::Completion>,
    _completion_event: Subscription,
    _command_removal: Subscription,
    _draft_observation: Subscription,
    composer_draft: composer_draft::State,
    ticket_context: ticket_context::State,
    _draft_quit: Subscription,
    _input_focus: Option<[Subscription; 2]>,
    draft_has_value: bool,
    folder_selected: bool,
    follow_ups: follow_up::FollowUps,
    attachments: Vec<std::path::PathBuf>,
    pending_attachments: Vec<std::path::PathBuf>,
    attaching: bool,
    file_drag: bool,
    attachment_error: Option<String>,
    unconfirmed_submission: Option<vorkspace_core::store::SubmissionReceipt>,
    reviewing_submission: bool,
    submission_recovery_epoch: u64,
    app_environment: std::collections::HashMap<String, String>,
    /// Bots' minion sessions. A minion's turns take its parent's context
    /// and capability from here as each turn starts, so turning Minions on
    /// or off reaches sessions that are already open. Never saved.
    pub(crate) minions: Option<crate::bots::MinionRegistry>,
    pub(crate) team_histories: Option<crate::bots::TeamHistories>,
    team_dispatch: bool,
    team_look: Option<(crate::bots::avatar::BotLookConfig, String)>,
    planning_orchestration: bool,
    pub(crate) orchestration_plan: Option<vorkspace_core::orchestration::Plan>,
    orchestration_error: Option<String>,
    pub(crate) orchestration_progress: Option<String>,
    status: String,
    pub(crate) activity: Option<String>,
    loaded: bool,
    history_unloaded: bool,
    history_partial: bool,
    pub(crate) allow_storage_paging: bool,
    _history_events: Subscription,
    loaded_history_revision: Option<i64>,
    history_actions: std::collections::VecDeque<history::HistoryAction>,
    pending_search: Option<(usize, String)>,
    loading: bool,
    pub(crate) busy: bool,
    /// The UI is idle; serialize the next launch behind the worker's last save.
    stopping: bool,
    notification_cancelled: bool,
    /// The store's saved-draft flag, until the conversation loads its own.
    pub(crate) draft_hint: bool,
    /// The provider model for the next turn; `None` uses its configured default.
    model: Option<String>,
    resolved_model: Option<String>,
    pub(crate) settings: Settings,
    pub(crate) bot_turn: u64,
    pub(crate) submission_generation: u64,
    bot_ui: bot::ChatState,
    project_appearance: vorkspace_core::project_appearance::ProjectAppearance,
    classic_layout: bool,
    catalog: Vec<Model>,
    catalog_epoch: u64,
    account_switching: bool,
    _usage_clock: gpui::Task<()>,
    _account_usage: Option<Subscription>,
    approvals: std::collections::VecDeque<Approval>,
    approval_ui: approvals::State,
    questions: std::collections::VecDeque<(
        Entity<crate::composer::questions::QuestionCard>,
        Vec<Subscription>,
    )>,
    controls: Option<std::sync::mpsc::Sender<Control>>,
    branch: Option<String>,
    git: Option<vorkspace_core::git::Snapshot>,
    git_picker: Option<Entity<GitPicker>>,
    git_events: Option<Subscription>,
    preparing_worktree: bool,
    context: Option<vorkspace_core::context::Usage>,
    context_hovered: bool,
    menu: Option<OpenMenu>,
    add_anchor: AnchorSlot,
    model_anchor: AnchorSlot,
    access_anchor: AnchorSlot,
    account_anchor: AnchorSlot,
    workspace_anchor: AnchorSlot,
    branch_anchor: AnchorSlot,
    context_anchor: AnchorSlot,
    compact_anchor: AnchorSlot,
    compact_focus: gpui::FocusHandle,
    composer_anchor: AnchorSlot,
    composer_motion: ComposerMotion,
    narrow: bool,
    // A split or review pane sits beside this chat; only then does a narrow
    // pane rest the composer where it docks.
    beside_pane: bool,
    cancel: Arc<AtomicBool>,
    /// Failed fresh app installations must not leave detached settings writes.
    settings_write_cancel: Arc<AtomicBool>,
    title_cancel: Option<Arc<AtomicBool>>,
    _submission: Subscription,
    response_events: Option<Subscription>,
    response_menu: Option<ResponseMenu>,
    response_request: Option<(usize, vorkspace_core::response::TransferKind, Instant)>,
}
impl Drop for Conversation {
    fn drop(&mut self) {
        let _ = self.composer_draft.flush();
        if !self.summary.cwd.starts_with("remote://") {
            self.cancel.store(true, Ordering::Relaxed);
        }
        self.cancel_title_generation();
    }
}
impl Conversation {
    /// Seed workspace metadata before an unloaded blank can render or submit.
    /// Loaded/running conversations keep their current agent settings.
    pub(crate) fn restore_worktree(
        &mut self,
        path: Option<String>,
        branch: Option<String>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if !self.loaded && self.settings.revision == 0 && self.settings.worktree_cwd != path {
            self.settings.worktree_cwd = path;
            self.git = self
                .settings
                .worktree_cwd
                .as_deref()
                .and_then(vorkspace_model::git_cache::get);
            self.branch = branch.or_else(|| self.git.as_ref().and_then(|s| s.current.clone()));
            self.sync_tool_paths(cx);
            cx.notify();
        } else if !self.loaded && branch.is_some() && self.branch != branch {
            self.branch = branch;
            cx.notify();
        }
        self.settings.worktree_cwd.clone()
    }
    pub(crate) fn settings_models(&self) -> Vec<(String, String)> {
        self.catalog
            .iter()
            .map(|model| {
                (
                    model.id.clone(),
                    model_display_name(
                        Kind::from_id(&self.summary.provider),
                        &model.id,
                        &model.name,
                    )
                    .to_string(),
                )
            })
            .collect()
    }
    pub(crate) fn set_classic_layout(&mut self, classic: bool, cx: &mut Context<Self>) {
        if self.classic_layout != classic {
            self.classic_layout = classic;
            cx.notify();
        }
    }
    pub(crate) fn set_beside_pane(&mut self, beside: bool, cx: &mut Context<Self>) {
        if self.beside_pane != beside {
            self.beside_pane = beside;
            cx.notify();
        }
    }
    pub(crate) fn set_project_appearance(
        &mut self,
        appearance: vorkspace_core::project_appearance::ProjectAppearance,
        cx: &mut Context<Self>,
    ) {
        if self.project_appearance != appearance {
            self.project_appearance = appearance;
            cx.notify();
        }
    }
    /// SessionCardPreview.tsx's compact model/status header and workspace details.
    /// Its latest message is saved as a draft, not sent (`session.draft`).
    pub(crate) fn has_saved_draft(&self) -> bool {
        if self.loaded {
            self.follow_ups.state.draft.is_some()
        } else {
            self.draft_hint
        }
    }
    pub(crate) fn needs_approval(&self) -> bool {
        !self.approvals.is_empty() || !self.questions.is_empty()
    }
    pub(crate) fn responding(&self) -> bool {
        self.busy && !self.stopping
    }
    pub(crate) fn sidebar_details(&self) -> Option<(SharedString, Option<String>)> {
        self.loaded
            .then(|| (self.model_label(), self.branch.clone()))
    }
    pub(crate) fn new(summary: Session, store: Database, cx: &mut Context<Self>) -> Self {
        #[cfg(test)]
        vorkspace_ui::clock::use_test_clock(cx);
        let input = cx.new(input::Input::composer);
        // A new tab shows its folder's checkout from the first frame; the
        // load below confirms it rather than popping it in.
        let git = (!summary.has_messages)
            .then(|| vorkspace_model::git_cache::get(&summary.cwd))
            .flatten();
        let completion = cx.new(|cx| {
            crate::composer::composer_completion::Completion::new(
                input.clone(),
                summary.cwd.clone(),
                cx,
            )
            .with_store(store.clone())
        });
        completion.update(cx, |completion, cx| {
            completion.set_provider(&summary.provider, cx)
        });
        let completion_event = cx.subscribe(
            &completion,
            |this, _, event: &crate::composer::composer_completion::ModePicked, cx| {
                if event.0 == "add-to-folder" {
                    this.request_folder(cx);
                } else {
                    this.enable_mode(event.0, cx);
                }
            },
        );
        let command_removal = cx.subscribe(&input, |this, _, event: &input::CommandRemoved, cx| {
            this.clear_mode(event.0, cx)
        });
        let draft_observation = cx.observe(&input, |this, input, cx| {
            let content = input.read(cx).content.clone();
            if this.ticket_context.edited(&content) && this.status == "Loading ticket context…" {
                this.status = "Ready".into();
                cx.notify();
            }
            let folder = vorkspace_core::composer::consume_command(&content, "add-to-folder");
            if folder.is_none() {
                this.folder_selected = false;
            }
            if !this.folder_selected
                && folder == Some("")
                && content.ends_with(' ')
                && !input.read(cx).is_composing()
            {
                input.update(cx, |input, cx| {
                    input.set_text(content.trim_end().into(), cx)
                });
                this.request_folder(cx);
            }
            let has_value = input.read(cx).has_message_text();
            if this.draft_has_value != has_value {
                this.draft_has_value = has_value;
                cx.notify();
            }
            this.open_typed_btw(cx);
            this.persist_composer(cx);
        });
        let submission = cx.subscribe(&input, |this, _, _: &input::Submitted, cx| this.send(cx));
        let review = cx.new(|_| {
            crate::git_tools::session_review::SessionReview::new(summary.id, summary.cwd.clone())
        });
        let review_events = cx.subscribe(&review, |this, review, event, cx| match event {
            crate::git_tools::session_review::SessionReviewEvent::Open { path } => {
                let (session, cwd) = review.read(cx).session();
                cx.emit(OpenSessionChanges(
                    crate::git_tools::review::ReviewTab::Session {
                        cwd: cwd.to_owned(),
                        session,
                        path: path.clone(),
                    },
                ));
            }
            // Undo and Keep may rewrite files; the checkout's views reread.
            crate::git_tools::session_review::SessionReviewEvent::Changed => {
                this.refresh_git(cx);
                cx.emit(SessionChanged);
            }
        });
        let review_observation = cx.observe(&review, |this, review, cx| {
            let shown = review.read(cx).shown();
            this.transcript
                .update(cx, |view, cx| view.set_review_shown(shown, cx));
        });
        let transcript = cx.new(|_| {
            let mut view = TranscriptView::new();
            view.identity(&summary.provider, Kind::display_label(&summary.provider));
            view.file_link_base = Some(summary.cwd.clone());
            view.set_review(review.clone().into());
            view
        });
        let history_events = cx.subscribe(
            &transcript,
            |this, transcript, _: &crate::transcript::StorageHistoryStale, cx| {
                this.pending_search = transcript.update(cx, |view, _| view.take_storage_search());
                this.history_unloaded = true;
                this.reload_history(cx);
            },
        );
        let account = crate::accounts::get(cx)
            .map(|center| {
                center
                    .read(cx)
                    .selected(Kind::from_id(&summary.provider), &summary.cwd)
            })
            .unwrap_or_default();
        let usage_clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                if this.update(cx, |this, cx| this.tick_usage(cx)).is_err() {
                    break;
                }
            }
        });
        let account_usage = crate::accounts::get(cx).map(|center| {
            cx.observe(&center, |this, _, cx| {
                if this.settings.usage_limit.is_some() {
                    this.tick_usage(cx);
                }
            })
        });
        let draft_state = composer_draft::State::new(store.clone(), summary.id);
        let draft_quit = cx.on_app_quit(|this, cx| {
            this.capture_composer(cx);
            let writer = this.composer_draft.writer.clone();
            cx.background_executor().spawn(async move {
                let _ = composer_draft::flush(&writer);
            })
        });
        let mut conversation = Self {
            editing_last_turn: Default::default(),
            btw: None,
            store,
            bot_turn: 0,
            submission_generation: 0,
            activity: None,
            settings_write_cancel: Arc::new(AtomicBool::new(false)),
            bot_ui: bot::ChatState::default(),
            transcript,
            review,
            _review_events: review_events,
            _review_observation: review_observation,
            input,
            completion,
            _completion_event: completion_event,
            _command_removal: command_removal,
            _draft_observation: draft_observation,
            composer_draft: draft_state,
            ticket_context: Default::default(),
            _draft_quit: draft_quit,
            _input_focus: None,
            draft_has_value: false,
            folder_selected: false,
            follow_ups: Default::default(),
            attachments: Vec::new(),
            pending_attachments: Vec::new(),
            attaching: false,
            file_drag: false,
            attachment_error: None,
            unconfirmed_submission: None,
            reviewing_submission: false,
            submission_recovery_epoch: 0,
            app_environment: Default::default(),
            minions: None,
            team_histories: None,
            team_dispatch: false,
            team_look: None,
            planning_orchestration: false,
            orchestration_plan: None,
            orchestration_error: None,
            orchestration_progress: None,
            status: "Loading conversation…".into(),
            loaded: false,
            history_unloaded: false,
            history_partial: false,
            allow_storage_paging: false,
            _history_events: history_events,
            loaded_history_revision: None,
            history_actions: Default::default(),
            pending_search: None,
            loading: false,
            busy: false,
            stopping: false,
            notification_cancelled: false,
            draft_hint: false,
            model: std::env::var(format!(
                "VORKSPACE_{}_MODEL",
                summary.provider.to_uppercase()
            ))
            .ok(),
            resolved_model: None,
            settings: Settings {
                account,
                access: default_access(cx),
                effort: preferred_effort(&summary.provider, cx),
                ..Default::default()
            },
            project_appearance: Default::default(),
            classic_layout: false,
            catalog: Vec::new(),
            catalog_epoch: 0,
            account_switching: false,
            _usage_clock: usage_clock,
            _account_usage: account_usage,
            approvals: Default::default(),
            approval_ui: Default::default(),
            questions: Default::default(),
            controls: None,
            branch: git.as_ref().and_then(|s| s.current.clone()),
            git,
            git_picker: None,
            git_events: None,
            preparing_worktree: false,
            context: None,
            context_hovered: false,
            menu: None,
            add_anchor: AnchorSlot::default(),
            model_anchor: AnchorSlot::default(),
            access_anchor: AnchorSlot::default(),
            account_anchor: AnchorSlot::default(),
            workspace_anchor: AnchorSlot::default(),
            branch_anchor: AnchorSlot::default(),
            context_anchor: AnchorSlot::default(),
            compact_anchor: AnchorSlot::default(),
            compact_focus: cx.focus_handle(),
            composer_anchor: AnchorSlot::default(),
            composer_motion: ComposerMotion::default(),
            narrow: false,
            beside_pane: false,
            cancel: Arc::new(AtomicBool::new(false)),
            title_cancel: None,
            _submission: submission,
            response_events: None,
            response_menu: None,
            response_request: None,
            summary,
            remote_state: remote::State::default(),
            remote_plans: remote_plans::State::default(),
        };
        conversation.load_composer(cx);
        conversation
    }
    pub(crate) fn reveal_search(&mut self, row: usize, query: String, cx: &mut Context<Self>) {
        if !self.loaded || self.history_unloaded {
            self.pending_search = Some((row, query));
            self.load(cx);
            return;
        }
        self.transcript.update(cx, |view, cx| {
            if self.history_partial {
                view.reveal_storage_search(row, &query, cx);
            } else {
                view.reveal_search(row, &query, cx);
            }
        });
    }
}

impl EventEmitter<SessionChanged> for Conversation {}
impl EventEmitter<SessionActivityChanged> for Conversation {}
impl EventEmitter<ChooseFolder> for Conversation {}
impl EventEmitter<OpenSessionChanges> for Conversation {}
impl EventEmitter<Transfer> for Conversation {}
impl EventEmitter<composer_features::StartOrchestration> for Conversation {}
impl EventEmitter<composer_features::StopOrchestration> for Conversation {}

impl Conversation {}

/// The access mode the last composer pick left as the default for new chats.
pub(crate) fn default_access(cx: &gpui::App) -> AccessMode {
    cx.try_global::<crate::settings::Preferences>()
        .and_then(|prefs| prefs.default_access.as_deref())
        .map(AccessMode::from_id)
        .unwrap_or_default()
}
pub(crate) fn remember_access(mode: AccessMode, cx: &mut gpui::App) {
    if cx.has_global::<crate::settings::Preferences>() {
        cx.update_global::<crate::settings::Preferences, _>(|prefs, _| {
            prefs.default_access = Some(mode.id().into());
        });
    }
}
fn is_effort_key(key: &str) -> bool {
    matches!(
        key,
        "effort" | "reasoning" | "reasoningEffort" | "reasoning_effort"
    )
}
pub(crate) fn preferred_effort(provider: &str, cx: &gpui::App) -> Option<String> {
    cx.try_global::<crate::settings::Preferences>()
        .and_then(|prefs| prefs.provider_efforts.get(provider).cloned())
}
fn effort_name(value: &str) -> String {
    match value {
        "default" => "Default",
        "none" => "None",
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra High",
        "max" => "Max",
        "ultra" => "Ultra",
        "ultracode" => "Ultracode",
        "ultrathink" => "Ultrathink",
        other => other,
    }
    .into()
}

fn model_control_toggle(control: &catalog::ModelControl) -> bool {
    control.choices.len() == 2
        && control.choices.iter().any(|(value, _)| value == "true")
        && control.choices.iter().any(|(value, _)| value == "false")
}

#[cfg(test)]
mod composer_motion_tests;
impl Render for Conversation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._input_focus.is_none() {
            let focus = self.input.focus_handle(cx);
            self._input_focus = Some([
                cx.on_focus(&focus, window, |_, _, cx| cx.notify()),
                cx.on_blur(&focus, window, |_, _, cx| cx.notify()),
            ]);
        }
        if self.response_events.is_none() {
            self.response_events = Some(cx.subscribe_in(
                &self.transcript,
                window,
                |this, _, request: &ResponseRequest, window, cx| {
                    this.response_request(request, window, cx)
                },
            ));
        }
        if self.user_read_only() {
            return self.render_team_history(window, cx);
        }
        if self.settings.bot.is_some() {
            return self.render_bot(window, cx);
        }
        if !self.summary.has_messages {
            return self.empty(window, cx);
        }
        self.attachment_drop_target("session-chat", cx)
            .relative()
            .capture_action(cx.listener(|this, _: &crate::shell::DismissMenu, _, cx| {
                if this.close_remote_plan(cx) {
                    cx.stop_propagation();
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .when_some(self.response_menu.as_ref(), |d, menu| {
                let panel =
                    menu_popover(&menu.menu, Anchor::Slot(menu.anchor.clone()), cx).side(Side::Top);
                d.child(
                    menu.regions
                        .iter()
                        .fold(panel, |panel, region| panel.ignore(region.clone())),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    self.transcript
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                ),
            )
            .child(
                // A column, as in a Bot chat, so the shared column's auto
                // margins centre it.
                div().flex_shrink_0().w_full().flex().flex_col().child(
                    composer_column()
                        .when_some(self.questions.front(), |d, (card, _)| d.child(card.clone()))
                        .child(self.composer_motion.clone().wrap(self.composer(window, cx))),
                ),
            )
            .when_some(self.btw.as_ref(), |d, sheet| d.child(sheet.clone()))
            .children(self.remote_plan_reader(cx))
            .into_any()
    }
}

// The composer's `h-6.5` toolbar pills share `bg-selection` and its hover.
fn tool_pill(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .group(id)
        .cursor(CursorStyle::Arrow)
        .relative()
        .flex_shrink_0()
        .h(px(26.))
        .px(px(6.))
        .rounded_md()
        .flex()
        .items_center()
        .gap_1()
        .bg(content(0.1))
        .text_color(content(1.))
        .hover(|s| s.bg(content(0.15)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}
/// The column a composer sits in, shared by sessions and Bots.
pub(super) fn composer_column() -> gpui::Div {
    div()
        .w_full()
        .max_w(px(COMPOSER_COLUMN))
        .mx_auto()
        .px(px(10.))
        .pb(px(6.))
        .flex()
        .flex_col()
}
/// BotComposer's quiet control under the box: plain until hovered or open.
pub(super) fn quiet_control(
    id: impl Into<gpui::ElementId>,
    open: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(Role::Button)
        .cursor(CursorStyle::Arrow)
        .flex_shrink_0()
        .h(px(26.))
        .min_w_0()
        .px_1p5()
        .gap_1p5()
        .rounded_md()
        .flex()
        .items_center()
        .text_size(px(12.))
        .text_color(content(if open { 1. } else { 0.55 }))
        .when(open, |d| d.bg(content(0.08)))
        .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}
// GitPickerTrigger: the quiet controls in the bar under the box.
fn bar_trigger(
    id: &'static str,
    path: &'static str,
    label: SharedString,
) -> gpui::Stateful<gpui::Div> {
    quiet_control(id, false)
        .group(id)
        .flex_shrink(1.)
        .child(
            icon(path, 14.)
                .text_color(content(0.55))
                .group_hover(id, |s| s.text_color(content(1.))),
        )
        .child(div().min_w_0().truncate().child(label))
}
/// The checked-out branch (or short detached commit) for `cwd`, read from the
/// repository's HEAD, including linked worktrees whose `.git` is a file.
pub(crate) fn git_branch(cwd: &Path) -> Option<String> {
    let root = cwd.ancestors().find(|d| d.join(".git").exists())?;
    let dot = root.join(".git");
    let git = if dot.is_dir() {
        dot
    } else {
        let link = std::fs::read_to_string(&dot).ok()?;
        root.join(link.strip_prefix("gitdir:")?.trim())
    };
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => branch.to_owned(),
        None => head.chars().take(7).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::git_branch;
    #[test]
    fn reads_branch_from_repositories_and_linked_worktrees() {
        let root = std::env::temp_dir().join(format!("vorkspace-branch-{}", std::process::id()));
        let repo = root.join("repo");
        let nested = repo.join("src/deep");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(git_branch(&nested).as_deref(), Some("feature/x"));

        let linked = root.join("linked");
        let meta = repo.join(".git/worktrees/linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", meta.display())).unwrap();
        std::fs::write(meta.join("HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(git_branch(&linked).as_deref(), Some("0123456"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod menu_tests;
