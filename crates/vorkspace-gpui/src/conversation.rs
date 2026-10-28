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
mod edit_last_turn;
mod follow_up;
mod history;
mod local_submissions;
mod notifications;
#[cfg(test)]
pub(crate) mod receive_tests;
mod remote;
pub(crate) mod remote_plans;
mod response_menu;
#[cfg(test)]
mod submission_tests;
mod ticket_context;
mod title;
use crate::clock::Instant;
use crate::{
    composer_motion::ComposerMotion,
    git_picker::{GitPicker, Mode as GitMode, Picked as GitPicked},
    input,
    pickers::{
        ModelList, ModelPicked, cached_model_name, model_display_name, model_name, remember_model,
    },
    popover::{
        Align, Anchor, AnchorSlot, Flyout, Menu, MenuAction, MenuEvent, MenuItem, MenuStyle, Side,
        menu_popover, popover,
    },
    response_actions::{Request as ResponseRequest, Transfer},
    runtime::{Database, locked, run_turn},
    theme::*,
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
pub(crate) struct OpenSessionChanges(pub(crate) crate::review::ReviewTab);
pub(crate) struct Conversation {
    editing_last_turn: edit_last_turn::State,
    btw: Option<Entity<btw::Btw>>,
    pub(crate) summary: Session,
    remote_state: remote::State,
    remote_plans: remote_plans::State,
    store: Database,
    pub(crate) transcript: Entity<TranscriptView>,
    pub(crate) review: Entity<crate::session_review::SessionReview>,
    _review_events: Subscription,
    _review_observation: Subscription,
    pub(crate) input: Entity<input::Input>,
    completion: Entity<crate::composer_completion::Completion>,
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
    questions:
        std::collections::VecDeque<(Entity<crate::questions::QuestionCard>, Vec<Subscription>)>,
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
                .and_then(crate::git_cache::get);
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
    pub(crate) fn preview(&self, done: bool) -> AnyElement {
        let kind = Kind::from_id(&self.summary.provider);
        let model = self.resolved_model.as_deref().or(self.model.as_deref());
        let name = self
            .catalog
            .iter()
            .find(|entry| Some(entry.id.as_str()) == model)
            .map(|entry| model_display_name(kind, &entry.id, &entry.name))
            .unwrap_or_else(|| model_name(kind, model));
        let cwd = self
            .settings
            .worktree_cwd
            .as_deref()
            .unwrap_or(&self.summary.cwd);
        let directory = std::env::var_os("HOME")
            .and_then(|home| {
                Path::new(cwd).strip_prefix(home).ok().map(|path| {
                    if path.as_os_str().is_empty() {
                        "~".to_owned()
                    } else {
                        format!("~/{}", path.display())
                    }
                })
            })
            .unwrap_or_else(|| cwd.to_owned());
        let linked =
            self.settings.worktree_cwd.is_some() || self.git.as_ref().is_some_and(|git| git.linked);
        let detail = |label: &'static str, value: String| {
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .w(px(72.))
                        .flex_shrink_0()
                        .text_color(content(0.45))
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(content(0.8))
                        .child(value),
                )
        };
        div()
            .px_3()
            .py_2p5()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(px(11.))
                            .text_color(content(0.55))
                            .child(provider_icon(&self.summary.provider, 14.))
                            .child(div().min_w_0().truncate().child(name)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(if self.needs_approval() {
                                gpui::rgb(0xfbbf24).into()
                            } else if self.responding() {
                                accent_color()
                            } else if done {
                                gpui::rgb(0x34d399).into()
                            } else {
                                content(0.45)
                            })
                            .child(if self.needs_approval() {
                                "Needs approval".to_owned()
                            } else if self.busy {
                                "Working…".to_owned()
                            } else if done {
                                "Done".to_owned()
                            } else if self.loading {
                                "Loading…".to_owned()
                            } else {
                                vorkspace_core::time::relative(
                                    self.summary.updated_at,
                                    vorkspace_core::time::now_ms(),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .mt_1p5()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(content(1.))
                    .child(self.display_title()),
            )
            .child(
                div()
                    .mt_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(content(0.08))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .when_some(self.branch.clone(), |d, branch| {
                        d.child(detail("Branch", branch))
                    })
                    .child(detail(
                        if linked { "Worktree" } else { "Directory" },
                        directory,
                    )),
            )
            .into_any_element()
    }

    pub(crate) fn new(summary: Session, store: Database, cx: &mut Context<Self>) -> Self {
        #[cfg(test)]
        crate::clock::use_test_clock(cx);
        let input = cx.new(input::Input::composer);
        // A new tab shows its folder's checkout from the first frame; the
        // load below confirms it rather than popping it in.
        let git = (!summary.has_messages)
            .then(|| crate::git_cache::get(&summary.cwd))
            .flatten();
        let completion = cx.new(|cx| {
            crate::composer_completion::Completion::new(input.clone(), summary.cwd.clone(), cx)
                .with_store(store.clone())
        });
        completion.update(cx, |completion, cx| {
            completion.set_provider(&summary.provider, cx)
        });
        let completion_event = cx.subscribe(
            &completion,
            |this, _, event: &crate::composer_completion::ModePicked, cx| {
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
        let review =
            cx.new(|_| crate::session_review::SessionReview::new(summary.id, summary.cwd.clone()));
        let review_events = cx.subscribe(&review, |this, review, event, cx| match event {
            crate::session_review::SessionReviewEvent::Open { path } => {
                let (session, cwd) = review.read(cx).session();
                cx.emit(OpenSessionChanges(crate::review::ReviewTab::Session {
                    cwd: cwd.to_owned(),
                    session,
                    path: path.clone(),
                }));
            }
            // Undo and Keep may rewrite files; the checkout's views reread.
            crate::session_review::SessionReviewEvent::Changed => {
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
    pub(crate) fn load(&mut self, cx: &mut Context<Self>) {
        if self.is_remote() {
            self.load_remote(cx);
            return;
        }
        if self.history_unloaded {
            self.reload_history(cx);
            return;
        }
        if self.history_partial {
            if let Some(search) = self
                .transcript
                .update(cx, |view, _| view.take_storage_search())
            {
                self.pending_search = Some(search);
            }
            self.history_unloaded = true;
            self.reload_history(cx);
            return;
        }
        if self.loading {
            return;
        }
        if self.loaded {
            self.refresh_if_stale(cx);
            return;
        }
        self.loading = true;
        self.refresh_catalog(cx);
        let store = self.store.clone();
        let id = self.summary.id;
        let cwd = self.summary.cwd.clone();
        let bot = self.settings.bot.is_some();
        let page_bot = bot && self.allow_storage_paging;
        let recovery_epoch = self.submission_recovery_epoch;
        let work = cx.background_executor().spawn(async move {
            let settings = locked(&store).and_then(|s| s.agent_settings(id));
            let execution_cwd = settings
                .as_ref()
                .ok()
                .and_then(|s| s.as_ref())
                .and_then(|s| s.worktree_cwd.as_deref())
                .unwrap_or(&cwd);
            let git = crate::git_cache::load(Path::new(execution_cwd)).ok();
            let rev = locked(&store)
                .and_then(|s| s.session_history_revision(id))
                .unwrap_or(0);
            (
                locked(&store)
                    .and_then(|s| {
                        if page_bot
                            && let Ok(Some(page)) = s.indexed_history_page(id, None, 10)
                            && page.before.is_some()
                        {
                            return Ok(history::InitialRows::Window(Box::new(page)));
                        }
                        Ok(history::InitialRows::Complete(s.load(id)?.rows))
                    })
                    .map(|rows| match rows {
                        history::InitialRows::Complete(rows) => history::InitialHistory::Complete(
                            crate::transcript::PreparedHistory::new(rows, bot),
                        ),
                        history::InitialRows::Window(mut page) => {
                            let prepared = crate::transcript::PreparedHistory::new(
                                std::mem::take(&mut page.rows),
                                false,
                            );
                            history::InitialHistory::Window(page, prepared)
                        }
                    }),
                git,
                settings,
                locked(&store).and_then(|s| s.context_usage(id)),
                locked(&store).and_then(|s| s.follow_ups(id)),
                locked(&store).and_then(|s| {
                    s.recover_local_submissions()?;
                    s.latest_unconfirmed_submission(id)
                }),
                rev,
            )
        });
        cx.spawn(async move |this, cx| {
            let (result, git, settings, context, follow_ups, unconfirmed, rev) = work.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.loaded_history_revision = Some(rev);
                this.branch = git.as_ref().and_then(|s| s.current.clone());
                this.git = git;
                this.context = context.ok().flatten();
                if this.submission_recovery_epoch == recovery_epoch {
                    match unconfirmed {
                        Ok(receipt) => this.unconfirmed_submission = receipt,
                        Err(error) => {
                            this.status = error;
                            return;
                        }
                    }
                }
                match follow_ups {
                    Ok(state) => {
                        this.follow_ups.persisted = state.revision;
                        this.follow_ups.state = state;
                        let draft = this.follow_ups.state.draft.clone();
                        if draft.is_some() {
                            this.summary.has_messages = true;
                        }
                        this.transcript
                            .update(cx, |view, cx| view.set_saved_draft(draft, cx));
                        if this.settings.bot.is_some() {
                            this.sync_bot_pending(cx);
                        }
                    }
                    Err(error) => {
                        this.status = error;
                        return;
                    }
                }
                // The composer may have a newer revision while history loads;
                // historical turn provenance still belongs to the loaded rows.
                if let Ok(Some(settings)) = &settings {
                    this.transcript
                        .update(cx, |view, _| view.turns = settings.turns.clone());
                    this.settings.turns = settings.turns.clone();
                }
                if let Ok(Some(mut settings)) = settings
                    && settings.revision >= this.settings.revision
                {
                    if !this.summary.has_messages
                        && settings.revision == 0
                        && settings.account.id == vorkspace_core::accounts::DEFAULT
                    {
                        settings.account = crate::accounts::get(cx)
                            .map(|center| {
                                center.read(cx).selected(
                                    Kind::from_id(&this.summary.provider),
                                    &this.summary.cwd,
                                )
                            })
                            .unwrap_or_default();
                    }
                    this.model = settings.model.clone();
                    let bot = this.settings.bot.take();
                    let environment = std::mem::take(&mut this.settings.environment);
                    this.settings = settings;
                    this.attach_account(cx);
                    this.settings.bot = bot;
                    this.settings.environment = environment;
                }
                match result {
                    Ok(history) => {
                        match history {
                            history::InitialHistory::Complete(history) => this
                                .transcript
                                .update(cx, |v, cx| v.install_prepared_history(history, cx)),
                            history::InitialHistory::Window(page, prepared) => {
                                this.history_partial = true;
                                this.transcript.update(cx, |v, cx| {
                                    v.install_storage_history(
                                        *page,
                                        prepared,
                                        this.store.clone(),
                                        id,
                                        cx,
                                    )
                                });
                            }
                        }
                        this.status = "Ready".into();
                        this.loaded = true;
                        if let Some(message) = this
                            .follow_ups
                            .state
                            .draft
                            .as_ref()
                            .and_then(|draft| this.bot_ui.accepted_drafts.get(&draft.id))
                            .cloned()
                        {
                            this.bot_draft_outbox_saved(&message, cx);
                        }
                        if let Some((row, query)) = this.pending_search.take() {
                            this.reveal_search(row, query, cx);
                        }
                        this.sync_review(cx);
                        this.run_history_actions(cx);
                        this.pump_follow_ups(cx);
                        if this.history_partial
                            && (!this.history_actions.is_empty()
                                || !this.follow_ups.state.messages.is_empty())
                        {
                            this.load(cx);
                        }
                    }
                    Err(e) => this.status = e,
                }
                cx.emit(SessionChanged);
                cx.notify();
            });
        })
        .detach();
    }
    pub(crate) fn refresh_if_stale(&mut self, cx: &mut Context<Self>) {
        if !self.loaded {
            self.load(cx);
            return;
        }
        if self.loading || self.busy || self.stopping {
            return;
        }
        // A partial transcript is refreshed when a cursor or mutation guard
        // detects a change; replacing it here would lose its reading anchor.
        if self.history_partial {
            return;
        }
        if self.history_unloaded {
            self.reload_history(cx);
            return;
        }
        if self.is_remote() {
            self.refresh_remote_snapshot(cx);
            return;
        }
        let id = self.summary.id;
        let store = self.store.clone();
        let current_rev = self.loaded_history_revision;
        let bot = self.settings.bot.is_some();
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            let rev = store.session_history_revision(id).unwrap_or(0);
            if current_rev == Some(rev) {
                return Ok::<_, String>(None);
            }
            let history = store.load(id)?;
            let turns = store.agent_settings(id)?.unwrap_or_default().turns;
            Ok(Some((
                crate::transcript::PreparedHistory::new(history.rows, bot),
                turns,
                rev,
            )))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(Some((history, turns, rev))) = result {
                    this.settings.turns = turns.clone();
                    this.transcript.update(cx, |view, cx| {
                        view.turns = turns;
                        view.restore_prepared_history(history, cx);
                    });
                    this.loaded_history_revision = Some(rev);
                    this.status = "Ready".into();
                    if this.follow_ups.state.paused
                        && this.follow_ups.state.messages.iter().any(|m| {
                            m.error
                                .as_deref()
                                .is_some_and(|e| e.contains("another window"))
                        })
                    {
                        this.follow_ups.state.paused = false;
                        for message in &mut this.follow_ups.state.messages {
                            if message
                                .error
                                .as_deref()
                                .is_some_and(|e| e.contains("another window"))
                            {
                                message.error = None;
                            }
                        }
                        this.save_follow_ups(cx);
                    }
                    this.sync_review(cx);
                    cx.emit(SessionChanged);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(crate) fn quick_submit(
        &mut self,
        prompt: String,
        settings: Settings,
        cx: &mut Context<Self>,
    ) {
        if self.editing_last_turn.pending() {
            self.defer_edit_transfer(settings, Some(prompt), cx);
            return;
        }
        if self.needs_history_restore() || self.stopping {
            self.defer_history_action(
                move |this, cx| {
                    if this.preserve_new_composer(cx) {
                        this.install_transfer(settings, None, cx);
                        this.enqueue_restored_prompt(prompt, cx);
                    } else {
                        this.quick_submit(prompt, settings, cx);
                    }
                },
                cx,
            );
            return;
        }
        self.loaded = true;
        self.model = settings.model.clone();
        self.settings = settings;
        self.input
            .update(cx, |input, cx| input.set_text(prompt, cx));
        self.send(cx);
    }
    fn execution_provider(&self) -> Result<Kind, String> {
        Kind::parse(&self.summary.provider).ok_or_else(|| {
            format!(
                "Provider {} is no longer supported. Saved history and drafts are retained.",
                self.summary.provider
            )
        })
    }
    fn send(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.execution_provider() {
            self.status = error;
            cx.notify();
            return;
        }
        if let Some(error) = &self.composer_draft.error {
            self.status = error.clone();
            cx.notify();
            return;
        }
        if self.is_remote() && self.remote_send_guard(cx) {
            return;
        }
        if self.needs_history_restore() {
            self.defer_history_action(|this, cx| this.send(cx), cx);
            return;
        }
        if self.submit_edited(cx) {
            return;
        }
        if self.handle_local_command(cx) {
            return;
        }
        if self.settings.bot.is_some() && self.draft_active(cx) {
            self.save_message_draft(cx);
            return;
        }
        if self.settings.bot.is_some() {
            if self.attaching
                || self.input.read(cx).is_composing()
                || self
                    .bot_ui
                    .direct_sending
                    .as_ref()
                    .is_some_and(|pending| !pending.4)
            {
                return;
            }
            let raw = self.input.read(cx).content.to_string();
            let attachments = self.attachments.clone();
            if !raw.trim().is_empty() || !attachments.is_empty() {
                // The outbox acknowledgement clears only this exact input. A
                // failed write or process exit before admission leaves a saved
                // composer instead of silently discarding the submission.
                self.capture_composer(cx);
                if let Err(error) = self.composer_draft.flush() {
                    self.status =
                        format!("Could not save composer: {error}. Your input was preserved.");
                    cx.notify();
                    return;
                }
                let selection = self.input.read(cx).selection_utf16();
                if self
                    .bot_ui
                    .direct_sending
                    .as_ref()
                    .is_some_and(|(_, previous, files, _, _)| {
                        previous != &raw || files != &attachments
                    })
                {
                    self.status = "The previous message was not queued. Restore its original input and send it to retry; your current input is preserved.".into();
                    cx.notify();
                    return;
                }
                let recovery = match self.pending_bot_submission(&raw, &attachments) {
                    Ok(recovery) => recovery,
                    Err(error) => {
                        self.status = format!(
                            "Could not verify the previous submission: {error}. Your input was preserved."
                        );
                        cx.notify();
                        return;
                    }
                };
                let text = self.consume_mode_command(raw.clone(), cx);
                let mode = if self.settings.orchestrator {
                    Some("orchestrator")
                } else if self.settings.plan {
                    Some("plan")
                } else if self.settings.operator {
                    Some("operator")
                } else {
                    None
                };
                let mode = recovery.as_ref().map_or(mode.map(str::to_owned), |record| {
                    record.payload["botSubmission"]["mode"]
                        .as_str()
                        .map(str::to_owned)
                });
                let queue = recovery.as_ref().map_or(self.bot_ui.queue, |record| {
                    record.payload["botSubmission"]["queue"]
                        .as_bool()
                        .unwrap_or(false)
                });
                let id = if let Some((id, _, _, _, _)) = &self.bot_ui.direct_sending {
                    id.clone()
                } else if let Some(record) = &recovery {
                    record.owner.clone()
                } else {
                    let id = vorkspace_core::bot::new_id();
                    let result = locked(&self.store).and_then(|store| {
                        let mut saved = store
                            .composer_draft(self.summary.id)?
                            .ok_or("Composer was not saved")?;
                        saved.payload["botSubmission"] =
                            serde_json::json!({"mode": mode, "queue": queue});
                        store.save_composer_recovery(
                            self.summary.id,
                            vorkspace_core::store::ComposerRecovery {
                                owner: id.clone(),
                                revision: saved.revision,
                                payload: saved.payload,
                            },
                        )
                    });
                    if let Err(error) = result {
                        self.status = format!(
                            "Could not preserve composer before sending: {error}. Your input was preserved."
                        );
                        cx.notify();
                        return;
                    }
                    id
                };
                self.bot_ui.queue = false;
                self.bot_ui.direct_sending =
                    Some((id.clone(), raw, attachments.clone(), selection, false));
                cx.emit(bot::Submitted {
                    draft_id: Some(id),
                    text,
                    attachments,
                    queue,
                    mode: mode.clone(),
                });
                if mode.is_some() {
                    self.clear_submitted_mode(cx);
                }
                cx.notify();
            }
            return;
        }
        if self.settings.worktree_removed {
            self.status = "Select a branch or worktree to continue this conversation.".into();
            cx.notify();
            return;
        }
        if !self.loaded || (self.preparing_worktree && !self.busy && !self.stopping) {
            return;
        }
        if self.draft_active(cx) {
            self.save_message_draft(cx);
            return;
        }
        if !self.folder_selected
            && vorkspace_core::composer::consume_command(
                &self.input.read(cx).content,
                "add-to-folder",
            )
            .is_some()
        {
            self.request_folder(cx);
            return;
        }
        if self.attaching
            || (!self.input.read(cx).has_message_text() && self.attachments.is_empty())
        {
            return;
        }
        if self.input.read(cx).is_composing() {
            return;
        }
        if self.prepare_ticket_context(cx) {
            return;
        }
        let raw = self
            .input
            .update(cx, |i, cx| i.take(cx))
            .unwrap_or_default();
        let raw = vorkspace_core::composer::consume_command(&raw, "add-to-folder")
            .map(str::to_owned)
            .unwrap_or(raw);
        self.folder_selected = false;
        let prompt = self.consume_mode_command(raw, cx);
        if prompt.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        self.completion
            .update(cx, |completion, cx| completion.dismiss(cx));
        let mode = if self.settings.orchestrator {
            Some("orchestrator")
        } else if self.settings.plan {
            Some("plan")
        } else if self.settings.operator {
            Some("operator")
        } else {
            None
        };
        if self.settings.pending_provider_switch.is_some() && self.busy {
            // Picking a provider only arms the change. Sending is what stops
            // the outgoing provider and commits the handoff.
            self.stop(cx);
        }
        if self.stopping
            || self.busy
            || self.settings.pending_provider_switch.is_some()
            || (!self.follow_ups.state.messages.is_empty() && !self.follow_ups.state.paused)
        {
            self.enqueue_follow_up(prompt, mode.map(str::to_owned), cx);
            if self.stopping || self.settings.pending_provider_switch.is_some() {
                // A fresh Send continues the conversation, while older queued
                // messages remain paused after Stop.
                if let Some(id) = self
                    .follow_ups
                    .state
                    .messages
                    .last()
                    .map(|message| message.id.clone())
                {
                    self.follow_ups.continuations.push_back(id);
                    if self.follow_ups.requested.is_none() {
                        self.follow_ups.requested = self.follow_ups.continuations.front().cloned();
                    }
                }
            }
            if mode.is_some() {
                self.clear_submitted_mode(cx);
            }
            return;
        }
        self.planning_orchestration = self.settings.orchestrator;
        self.orchestration_plan = None;
        self.orchestration_error = None;
        self.launch_prompt(prompt, cx);
        if self.busy && mode.is_some() {
            self.clear_submitted_mode(cx);
        }
    }
    fn launch_prompt(&mut self, prompt: String, cx: &mut Context<Self>) {
        let kind = match self.execution_provider() {
            Ok(kind) => kind,
            Err(error) => {
                self.status = error;
                cx.notify();
                return;
            }
        };
        if self.stopping {
            let mode = if self.settings.orchestrator {
                Some("orchestrator")
            } else if self.settings.plan {
                Some("plan")
            } else if self.settings.operator {
                Some("operator")
            } else {
                None
            };
            self.enqueue_follow_up(prompt, mode.map(str::to_owned), cx);
            if let Some(id) = self
                .follow_ups
                .state
                .messages
                .last()
                .map(|message| message.id.clone())
            {
                self.follow_ups.continuations.push_back(id);
                if self.follow_ups.requested.is_none() {
                    self.follow_ups.requested = self.follow_ups.continuations.front().cloned();
                }
            }
            return;
        }
        if !self.history_ready() {
            self.defer_history_action(move |this, cx| this.launch_prompt(prompt, cx), cx);
            return;
        }
        if self.account_switching {
            return;
        }
        if self.settings.account.id != vorkspace_core::accounts::DEFAULT
            && crate::accounts::get(cx)
                .is_some_and(|center| !center.read(cx).book.exists(kind, &self.settings.account.id))
        {
            self.account_unavailable(cx);
            return;
        }
        self.settings.usage_limit = None;
        self.attach_account(cx);
        if !self.is_remote() {
            self.generate_title(&prompt, cx);
        }
        let model_label = self.model_label();
        self.sync_tool_paths(cx);
        self.transcript
            .update(cx, |v, _| v.identity(&self.summary.provider, &model_label));
        self.begin_turn(cx);
        let generation = self.submission_generation;
        let source_provider = self.summary.provider.clone();
        let source_account = self.settings.account.id.clone();
        self.status = if self.settings.pending_provider_switch.is_some() {
            "Preparing handoff…"
        } else {
            "Connecting…"
        }
        .into();
        self.cancel = Arc::new(AtomicBool::new(false));
        let summary = self.summary.clone();
        let store = self.store.clone();
        let cancel = self.cancel.clone();
        let mut settings = self.settings.clone();
        settings.ticket_context = self.ticket_context.ready.take().map(Into::into);
        if let Some(model) = self.model_info() {
            for control in &model.controls {
                settings
                    .model_settings
                    .entry(control.id.clone())
                    .or_insert_with(|| control.current.clone());
            }
        }
        settings.turn_model = Some(vorkspace_core::agent::TurnInfo {
            provider: self.summary.provider.clone(),
            model: model_label.to_string(),
            model_id: self
                .model_info()
                .map(|model| model.id.clone())
                .or(self.model.clone()),
            started_at: Some(vorkspace_core::time::now_ms()),
            ..Default::default()
        });
        if !self.is_remote() && settings.prepared_prompt.is_none() {
            let prepared = self.completion.read(cx).prepare_prompt(&prompt);
            if prepared != prompt {
                settings.prepared_prompt = Some(prepared);
            }
        }
        settings.disable_claude_hooks = cx
            .try_global::<crate::settings::Preferences>()
            .is_some_and(|p| !p.claude_hooks);
        settings.model = self.model.clone();
        settings.environment.extend(self.app_environment.clone());
        if settings.bot.is_none()
            && settings.remote_action.as_ref().is_none_or(|action| {
                (action["type"] == "send"
                    && action["draftBlockId"].is_null()
                    && action["planBlockId"].is_null())
                    || action["type"] == "draft"
            })
        {
            settings
                .attachments
                .extend(std::mem::take(&mut self.attachments));
        }
        if settings
            .remote_action
            .as_ref()
            .is_some_and(|action| action["planBlockId"].is_string())
        {
            settings.attachments.clear();
        }
        self.preparing_worktree = settings.new_worktree && settings.worktree_cwd.is_none();
        if self.preparing_worktree {
            settings.worktree_base = settings.worktree_base.or(self.branch.clone());
            self.status = "Creating worktree…".into();
        }
        if settings.bot.is_none()
            && settings
                .remote_action
                .as_ref()
                .is_none_or(|action| action["type"] == "send")
        {
            self.transcript
                .update(cx, |view, cx| view.submit_prompt(&prompt, &settings, cx));
        }
        if settings.orchestrator {
            settings.plan = true;
            settings.operator = false;
            self.settings.orchestrator = false;
        }
        if self.settings.handoff.take().is_some() {
            self.save_settings(cx);
        }
        let (controls, control_rx) = std::sync::mpsc::channel();
        self.controls = Some(controls);
        self.approvals.clear();
        self.questions.clear();
        let (tx, rx) = async_channel::bounded(8);
        let turn_model_id = settings
            .turn_model
            .as_ref()
            .and_then(|turn| turn.model_id.clone())
            .or_else(|| settings.model.clone());
        let remote_client = self
            .is_remote()
            .then(|| self.remote_state.client.clone())
            .flatten();
        cx.background_executor()
            .spawn(async move {
                if let Some(client) = remote_client {
                    crate::runtime::remote::run_with_client(
                        client, summary, prompt, settings, control_rx, store, cancel, tx,
                    );
                } else {
                    run_turn(summary, prompt, settings, control_rx, store, cancel, tx);
                }
            })
            .detach();
        self.receive_turn_updates(
            rx,
            generation,
            source_provider,
            source_account,
            turn_model_id,
            cx,
        );
        cx.emit(SessionChanged);
        cx.notify();
    }
    fn receive_turn_updates(
        &mut self,
        rx: async_channel::Receiver<crate::runtime::Update>,
        generation: u64,
        source_provider: String,
        source_account: String,
        mut turn_model_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let mut completed = false;
            while let Ok(update) = rx.recv().await {
                let finished = update.finished;
                completed |= finished;
                if this
                    .update(cx, |this, cx| {
                        if this.submission_generation != generation {
                            return;
                        }
                        let stopping = this.stopping;
                        // The final update settles accepted rows and storage.
                        // Ignore streaming notifications after the user stops.
                        if stopping && !finished {
                            return;
                        }
                        let source_selected = this.summary.provider == source_provider
                            && this.settings.account.id == source_account;
                        if let Some(cwd) = update.worktree_cwd
                            && (this.settings.worktree_cwd.as_ref() != Some(&cwd)
                                || this.settings.new_worktree)
                        {
                            this.preparing_worktree = false;
                            this.settings.worktree_cwd = Some(cwd);
                            this.settings.new_worktree = false;
                            this.save_settings(cx);
                            this.refresh_git(cx);
                        }
                        if let Some(commands) = update.commands.filter(|_| source_selected) {
                            this.completion.update(cx, |completion, cx| {
                                completion.set_native_commands(commands, cx)
                            });
                        }
                        let activity_changed = this.activity != update.activity;
                        this.activity = update.activity;
                        let needed_approval = this.needs_approval();
                        let incoming_notification_input = !stopping
                            && (!update.approvals.is_empty() || !update.questions.is_empty());
                        this.transcript.update(cx, |v, cx| {
                            if let Some(len) = update.source_len {
                                v.patch_snapshot(update.start, update.rows, len, cx);
                            } else {
                                v.patch(update.start, update.rows, cx);
                            }
                            if stopping {
                                v.stop_streaming(cx);
                            }
                        });
                        if let Some(id) = update.provider_turn_id {
                            this.transcript
                                .update(cx, |view, _| view.record_provider_turn(id));
                        }
                        if let Some(id) = update.provider_id {
                            if let Some(pending) = this.settings.pending_provider_switch.as_mut()
                                && pending.from == source_provider
                                && pending.account.id == source_account
                            {
                                pending.provider_id = Some(id);
                            } else if source_selected {
                                this.summary.provider_id = Some(id);
                            }
                        }
                        if let Some(handoff) = update.handoff {
                            this.settings.pending_provider_switch = None;
                            this.settings.handoff = handoff;
                            this.save_settings(cx);
                        }
                        if let Some(model) = update.config_model {
                            if let Some(pending) = this.settings.pending_provider_switch.as_mut()
                                && pending.from == source_provider
                                && pending.account.id == source_account
                            {
                                pending.model = Some(model);
                            } else if source_selected {
                                this.model = Some(model.clone());
                                this.settings.model = Some(model);
                            }
                        }
                        if let Some(settings) = update.config_settings {
                            if let Some(pending) = this.settings.pending_provider_switch.as_mut()
                                && pending.from == source_provider
                                && pending.account.id == source_account
                            {
                                pending.model_settings.extend(settings);
                            } else if source_selected {
                                this.settings.model_settings.extend(settings);
                            }
                            this.save_settings(cx);
                        }
                        if let Some(model) = update.model {
                            turn_model_id = Some(model.clone());
                            let name: SharedString = this
                                .catalog
                                .iter()
                                .find(|entry| source_selected && entry.id == model)
                                .map(|entry| {
                                    let name = model_display_name(
                                        Kind::from_id(&source_provider),
                                        &entry.id,
                                        &entry.name,
                                    );
                                    name.strip_prefix("Claude ")
                                        .unwrap_or(&name)
                                        .to_owned()
                                        .into()
                                })
                                .unwrap_or_else(|| {
                                    model_name(Kind::from_id(&source_provider), Some(&model))
                                });
                            if source_selected {
                                this.resolved_model = Some(model);
                            }
                            this.transcript
                                .update(cx, |v, _| v.identity(&source_provider, &name));
                        }
                        if let Some(context) = update.context.filter(|_| source_selected) {
                            this.context = Some(context);
                        }
                        if let Some(metrics) = update.metrics {
                            this.transcript.update(cx, |v, _| v.metrics(metrics));
                        }
                        if update.turn_ready && !stopping {
                            this.follow_ups.ready = true;
                        }
                        this.apply_receipts(
                            update.delivered,
                            update.follow_up_delivered,
                            update.follow_up_errors,
                            cx,
                        );
                        if !stopping || finished {
                            this.status = update.status;
                        }
                        if !stopping {
                            this.approvals.extend(update.approvals);
                        }
                        for id in update.approvals_resolved {
                            this.approvals.retain(|approval| approval.id != id);
                        }
                        for id in update.questions_resolved {
                            this.questions.retain(|(card, _)| card.read(cx).id() != id);
                        }
                        for request in update.questions.into_iter().filter(|_| !stopping) {
                            let card =
                                cx.new(|cx| crate::questions::QuestionCard::new(request, cx));
                            let events = cx.subscribe(
                                &card,
                                |this, _, reply: &crate::questions::Answered, cx| {
                                    if this
                                        .questions
                                        .front()
                                        .is_none_or(|(card, _)| card.read(cx).id() != reply.id)
                                    {
                                        return;
                                    }
                                    let before = this.needs_approval();
                                    let decision = reply
                                        .answers
                                        .clone()
                                        .map_or(Decision::Deny, Decision::Answer);
                                    if this.controls.as_ref().is_none_or(|controls| {
                                        controls
                                            .send(Control {
                                                id: reply.id.clone(),
                                                decision,
                                            })
                                            .is_err()
                                    }) {
                                        this.status =
                                            "The provider is no longer waiting for this answer"
                                                .into();
                                        cx.notify();
                                        return;
                                    }
                                    this.questions.pop_front();
                                    this.status = if !this.approvals.is_empty() {
                                        "Waiting for approval"
                                    } else if !this.questions.is_empty() {
                                        "Waiting for your answer"
                                    } else {
                                        "Working…"
                                    }
                                    .into();
                                    if before != this.needs_approval() {
                                        cx.emit(SessionChanged);
                                    }
                                    cx.notify();
                                },
                            );
                            let keep_open = cx.subscribe(
                                &card,
                                |this, _, request: &crate::questions::KeptOpen, cx| {
                                    if let Some(controls) = &this.controls {
                                        let _ = controls.send(Control {
                                            id: request.id.clone(),
                                            decision: Decision::KeepOpen,
                                        });
                                    }
                                    cx.emit(SessionChanged);
                                    cx.notify();
                                },
                            );
                            this.questions.push_back((card, vec![events, keep_open]));
                            this.status = "Waiting for your answer".into();
                        }
                        if !finished && !this.approvals.is_empty() {
                            this.status = "Waiting for approval".into();
                        } else if !finished && !this.questions.is_empty() {
                            this.status = "Waiting for your answer".into();
                        }
                        if finished {
                            if this.is_remote() {
                                this.settings.remote_action = None;
                                this.remote_state.refreshing = false;
                            }
                            this.summary.updated_at = vorkspace_core::time::now_ms();
                            this.busy = false;
                            this.stopping = false;
                            if this.is_remote() {
                                this.refresh_remote_snapshot(cx);
                            }
                            this.preparing_worktree = false;
                            if !stopping && let Some(limit) = update.usage_limit {
                                this.settings.usage_limit = Some(limit);
                            }
                            if !this.is_remote() {
                                this.detect_usage_limit(cx);
                            }
                            this.review
                                .update(cx, |review, cx| review.set_busy(false, cx));
                            this.follow_ups.ready = false;
                            if this.follow_ups.resuming {
                                this.follow_ups.resuming = false;
                                if !stopping && this.status == "Ready" {
                                    this.follow_ups.state.paused = false;
                                    for message in &mut this.follow_ups.state.messages {
                                        message.error = None;
                                    }
                                    this.save_follow_ups(cx);
                                }
                            }
                            if let Some(id) = this.follow_ups.sending.clone() {
                                this.fail_follow_up(
                                    &id,
                                    "The turn ended before this message was accepted".into(),
                                    cx,
                                );
                            }
                            if this.status != "Ready" && !this.follow_ups.state.messages.is_empty()
                            {
                                this.follow_ups.state.paused = true;
                                this.save_follow_ups(cx);
                            }
                            this.approvals.clear();
                            this.questions.clear();
                            this.controls = None;
                            this.refresh_submission_recovery(cx);
                            this.transcript.update(cx, |v, cx| v.streaming(false, cx));
                            let model_id = turn_model_id.clone();
                            this.transcript.update(cx, |view, _| {
                                if let Some(turn) = view.active_turn()
                                    && let Some(info) = view.turns.get_mut(&turn)
                                {
                                    info.model_id = model_id;
                                }
                            });
                            if stopping {
                                this.planning_orchestration = false;
                                this.orchestration_progress = None;
                            } else {
                                this.finish_orchestration_plan(cx);
                            }
                            this.settings.turns = this.transcript.read(cx).turns.clone();
                            this.save_settings(cx);
                            this.refresh_git(cx);
                            this.run_history_actions(cx);
                            cx.emit(SessionChanged);
                        } else if incoming_notification_input
                            || needed_approval != this.needs_approval()
                        {
                            cx.emit(SessionChanged);
                        }
                        if activity_changed {
                            cx.emit(SessionActivityChanged);
                        }
                        this.pump_follow_ups(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                // recv().await need not suspend: a producer can refill even a
                // bounded channel while we patch. Return to the native event
                // loop between atomic updates so input and frames can run.
                // A positive timer also avoids immediately requeueing a ready
                // continuation onto the main dispatch queue.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1))
                    .await;
            }
            if !completed {
                let _ = this.update(cx, |this, cx| this.turn_updates_closed(generation, cx));
            }
        })
        .detach();
    }
    /// The final update repeats the turn's initial receipt beside any steers.
    /// A steer is always accepted after its turn began, so its receipt wins.
    pub(super) fn apply_receipts(
        &mut self,
        delivered: Option<String>,
        follow_ups: Vec<String>,
        errors: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = delivered {
            if self.settings.bot.is_some() {
                self.bot_ui.delivered = Some(id);
                self.sync_bot_pending(cx);
            } else {
                self.follow_up_delivered(&id, cx);
            }
        }
        for id in follow_ups {
            self.follow_up_delivered(&id, cx);
        }
        for (id, error) in errors {
            self.fail_follow_up(&id, error, cx);
        }
    }
    fn turn_updates_closed(&mut self, generation: u64, cx: &mut Context<Self>) {
        // The sender can disappear while unwinding, without a final Update.
        // A superseded worker must never release the replacement turn's UI.
        if self.submission_generation != generation || (!self.busy && !self.stopping) {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.busy = false;
        self.stopping = false;
        self.preparing_worktree = false;
        self.planning_orchestration = false;
        self.orchestration_progress = None;
        self.review
            .update(cx, |review, cx| review.set_busy(false, cx));
        self.follow_ups.ready = false;
        self.follow_ups.resuming = false;
        self.follow_ups.requested = None;
        self.follow_ups.continuations.clear();
        if let Some(id) = self.follow_ups.sending.clone()
            && self.settings.bot.is_some()
        {
            self.fail_follow_up(&id, UNCONFIRMED_TURN.into(), cx);
        }
        if let Some(id) = self.follow_ups.sending.take()
            && let Some(message) = self
                .follow_ups
                .state
                .messages
                .iter_mut()
                .find(|m| m.id == id)
        {
            message.error = Some(UNCONFIRMED_TURN.into());
        }
        // Persistence callbacks pump the queue too. Publish the paused state
        // and clear explicit-send markers before scheduling any of those reads.
        self.follow_ups.state.paused = true;
        self.save_follow_ups(cx);
        self.approvals.clear();
        self.questions.clear();
        self.controls = None;
        self.status = UNCONFIRMED_TURN.into();
        self.transcript
            .update(cx, |view, cx| view.streaming(false, cx));
        self.settings.turns = self.transcript.read(cx).turns.clone();
        self.save_settings(cx);
        self.refresh_submission_recovery(cx);
        if self.is_remote() {
            self.settings.remote_action = None;
            self.remote_state.refreshing = false;
            self.refresh_remote_snapshot(cx);
        }
        cx.emit(SessionChanged);
        cx.notify();
    }
    fn begin_turn(&mut self, cx: &mut Context<Self>) {
        self.submission_generation = self.submission_generation.wrapping_add(1);
        if !self.summary.has_messages {
            self.composer_motion.capture(self.composer_anchor.get());
        }
        self.activity = None;
        self.busy = true;
        self.stopping = false;
        self.notification_cancelled = false;
        self.review
            .update(cx, |review, cx| review.set_busy(true, cx));
        self.follow_ups.ready = false;
        self.bot_turn = self
            .bot_turn
            .wrapping_add(1)
            .max(vorkspace_core::time::now_ms() & 0xffff_ffff);
        self.summary.updated_at = vorkspace_core::time::now_ms();
        self.transcript.update(cx, |v, cx| v.streaming(true, cx));
        self.summary.has_messages = true;
    }
}

impl EventEmitter<SessionChanged> for Conversation {}
impl EventEmitter<SessionActivityChanged> for Conversation {}
impl EventEmitter<ChooseFolder> for Conversation {}
impl EventEmitter<OpenSessionChanges> for Conversation {}
impl EventEmitter<Transfer> for Conversation {}
impl EventEmitter<composer_features::StartOrchestration> for Conversation {}
impl EventEmitter<composer_features::StopOrchestration> for Conversation {}

impl Conversation {
    pub(crate) fn install_transfer(
        &mut self,
        settings: Settings,
        prompt: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.editing_last_turn.pending() {
            self.defer_edit_transfer(settings, prompt, cx);
            return;
        }
        if self.needs_history_restore() || self.stopping {
            self.defer_history_action(
                move |this, cx| {
                    // Bot callers may configure their live context immediately
                    // after installing a transfer. Keep that context at replay.
                    let bot = this.settings.bot.take();
                    let environment = std::mem::take(&mut this.settings.environment);
                    let mut settings = settings;
                    if settings.bot.is_none() {
                        settings.bot = bot;
                    }
                    settings.environment.extend(environment);
                    match prompt {
                        Some(prompt) if this.preserve_new_composer(cx) => {
                            this.install_transfer(settings, None, cx);
                            this.enqueue_restored_prompt(prompt, cx);
                        }
                        prompt => this.install_transfer(settings, prompt, cx),
                    }
                },
                cx,
            );
            return;
        }
        self.model = settings.model.clone();
        self.settings = settings;
        self.attach_account(cx);
        self.loaded = true;
        self.status = "Ready".into();
        self.refresh_catalog(cx);
        self.refresh_git(cx);
        if let Some(prompt) = prompt {
            self.input.update(cx, |i, cx| i.set_text(prompt, cx));
            self.send(cx);
        } else {
            cx.notify();
        }
    }
    fn close_response_menu(&mut self, cx: &mut Context<Self>) {
        self.response_menu = None;
        self.response_request = None;
        self.transcript.update(cx, |view, cx| {
            view.target_action = None;
            cx.notify();
        });
        cx.notify();
    }
    fn response_request(
        &mut self,
        request: &ResponseRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.history_partial && !matches!(request, ResponseRequest::QuoteSelection { .. }) {
            self.status = "Loading complete history; please retry the action".into();
            self.load(cx);
            return;
        }
        match request {
            ResponseRequest::RemotePlan { id, text, build } => {
                self.remote_plan_request(id, text, *build, cx)
            }
            ResponseRequest::QuoteSelection { text } => self.bot_quote(text, window, cx),
            ResponseRequest::EditLastTurn { turn } => self.begin_edit_last_turn(*turn, window, cx),
            ResponseRequest::Draft { send } => self.draft_action(*send, cx),
            ResponseRequest::EditDraft {
                id,
                edit_id,
                text,
                finish,
                cancel,
            } => self.edit_saved_draft(id, edit_id, text, *finish, *cancel, cx),
            ResponseRequest::SaveNote {
                turn,
                text,
                selection,
            } => {
                let turn = *turn;
                let note = vorkspace_core::response::Note {
                    id: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as u64,
                    title: {
                        let title = self.display_title();
                        if *selection || title.trim().is_empty() || title == "New session" {
                            vorkspace_core::notes::title(text)
                        } else {
                            title
                        }
                    },
                    body: text.clone(),
                    tags: Vec::new(),
                    source_session: self.summary.id,
                    source_cwd: self.summary.cwd.clone(),
                };
                let store = self.store.clone();
                let work = cx
                    .background_executor()
                    .spawn(async move { locked(&store)?.save_note(&note) });
                cx.spawn(async move |this, cx| {
                    let result = work.await;
                    let _ = this.update(cx, |this, cx| {
                        this.transcript
                            .update(cx, |view, cx| view.note_result(turn, result, cx))
                    });
                })
                .detach();
            }
            ResponseRequest::Target { turn, kind, anchor } => {
                if self
                    .response_request
                    .is_some_and(|(at, k, _)| at == *turn && k == *kind)
                {
                    self.close_response_menu(cx);
                    return;
                }
                let turn = *turn;
                let kind = *kind;
                let token = Instant::now();
                let anchor = anchor.clone();
                self.menu = None;
                self.git_picker = None;
                self.response_request = Some((turn, kind, token));
                self.transcript.update(cx, |view, cx| {
                    view.target_action = Some((turn, kind));
                    cx.notify();
                });
                self.open_response_menu(turn, kind, token, anchor, window, cx);
                cx.notify();
            }
        }
    }
    fn finish_response_target(
        &mut self,
        turn: usize,
        kind: vorkspace_core::response::TransferKind,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let Ok(target) = serde_json::from_str::<vorkspace_core::response::Target>(id) else {
            return;
        };
        let from = self
            .transcript
            .read(cx)
            .turns
            .get(&turn)
            .map(|t| t.provider.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.summary.provider)
            .to_owned();
        let (rows, through) = self.transcript.read(cx).response_rows(turn);
        let transfer = Transfer {
            kind,
            target,
            from,
            turn: rows,
            through,
            cwd: self.summary.cwd.clone(),
            title: self.summary.title.clone(),
            settings: self.settings.clone(),
        };
        self.close_response_menu(cx);
        cx.emit(transfer);
    }
    pub(crate) fn worktree_deleted(
        &mut self,
        summary: Session,
        settings: Settings,
        cx: &mut Context<Self>,
    ) {
        self.summary = summary;
        self.settings = settings;
        self.context = None;
        self.status = "Worktree deleted".into();
        self.refresh_git(cx);
        cx.emit(SessionChanged);
        cx.notify();
    }
    fn execution_cwd(&self) -> &str {
        if self.settings.worktree_removed {
            return &self.summary.cwd;
        }
        self.settings
            .worktree_cwd
            .as_deref()
            .unwrap_or(&self.summary.cwd)
    }
    fn sync_tool_paths(&mut self, cx: &mut Context<Self>) {
        let cwd = self.execution_cwd().to_owned();
        self.transcript.update(cx, |view, cx| {
            if view.file_link_base.as_deref() != Some(cwd.as_str()) {
                view.file_link_base = Some(cwd);
                cx.notify();
            }
        });
    }
    /// The review card follows the checkout and rereads its recorded turn.
    pub(crate) fn sync_review(&mut self, cx: &mut Context<Self>) {
        self.sync_tool_paths(cx);
        if self.settings.bot.is_some() || self.is_remote() {
            return;
        }
        let cwd = self.execution_cwd().to_owned();
        self.review.update(cx, |review, cx| {
            if review.session().1 == cwd {
                review.refresh(cx);
            } else {
                review.set_cwd(cwd, cx);
            }
        });
    }
    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        self.sync_review(cx);
        let cwd = self.execution_cwd().to_owned();
        let work_cwd = cwd.clone();
        let work = cx
            .background_executor()
            .spawn(async move { crate::git_cache::load(Path::new(&work_cwd)).ok() });
        cx.spawn(async move |this, cx| {
            let git = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.execution_cwd() == cwd {
                    this.branch = git.as_ref().and_then(|s| s.current.clone());
                    this.git = git;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn prepare_worktree(&mut self, cx: &mut Context<Self>) {
        self.preparing_worktree = true;
        self.status = "Creating worktree…".into();
        let cwd = self.summary.cwd.clone();
        let base = self
            .settings
            .worktree_base
            .clone()
            .or(self.branch.clone())
            .unwrap_or_else(|| "HEAD".into());
        let id = self.summary.id;
        let work = cx
            .background_executor()
            .spawn(async move { vorkspace_core::git::create_worktree(Path::new(&cwd), &base, id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.preparing_worktree = false;
                match result {
                    Ok(path) => {
                        this.settings.worktree_cwd = Some(path.to_string_lossy().into_owned());
                        this.settings.new_worktree = false;
                        this.save_settings(cx);
                        this.refresh_git(cx);
                        if this.follow_ups.state.messages.is_empty() {
                            this.send(cx);
                        } else {
                            this.pump_follow_ups(cx);
                        }
                    }
                    Err(error) => {
                        this.status = error.clone();
                        if let Some(message) = this.follow_ups.state.messages.first() {
                            let id = message.id.clone();
                            this.fail_follow_up(&id, error, cx);
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn open_git(&mut self, mode: GitMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.preparing_worktree {
            return;
        }
        if self.git_picker.take().is_some() {
            self.git_events = None;
            window.focus(&self.input.focus_handle(cx), cx);
            cx.notify();
            return;
        }
        self.menu = None;
        let cwd = self.execution_cwd().into();
        let selected = if mode == GitMode::Base {
            self.settings.worktree_base.clone().or(self.branch.clone())
        } else {
            self.branch.clone()
        };
        let picker = cx.new(|cx| GitPicker::new(cwd, mode, selected, cx));
        self.git_events =
            Some(
                cx.subscribe_in(&picker, window, |this, _, event: &GitPicked, window, cx| {
                    match event {
                        GitPicked::Changed => {
                            if this.settings.worktree_removed {
                                this.settings.worktree_removed = false;
                                this.settings.worktree_cwd = None;
                                this.save_settings(cx);
                            }
                            this.refresh_git(cx);
                        }
                        GitPicked::Base(base) => {
                            this.settings.worktree_base = Some(base.clone());
                            this.save_settings(cx);
                        }
                        GitPicked::Worktree(path) => {
                            if !this.use_existing_worktree(path, cx) {
                                return;
                            }
                        }
                        GitPicked::Dismissed => {}
                    }
                    this.git_picker = None;
                    this.git_events = None;
                    window.focus(&this.input.focus_handle(cx), cx);
                    cx.notify();
                }),
            );
        window.focus(&picker.read(cx).focus(cx), cx);
        self.git_picker = Some(picker);
        cx.notify();
    }
    fn use_existing_worktree(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        if self.summary.has_messages && !self.settings.worktree_removed {
            return false;
        }
        self.settings.worktree_removed = false;
        self.settings.worktree_cwd = (vorkspace_core::files::path_key(path)
            != vorkspace_core::files::path_key(&self.summary.cwd))
        .then(|| path.to_owned());
        self.settings.new_worktree = false;
        self.save_settings(cx);
        self.refresh_git(cx);
        true
    }
    fn refresh_catalog(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let kind = Kind::from_id(&self.summary.provider);
        self.attach_account(cx);
        self.catalog_epoch += 1;
        let epoch = self.catalog_epoch;
        let account = self.settings.account.clone();
        let work = cx.background_executor().spawn({
            let remote_project = self.is_remote().then(|| self.summary.cwd.clone());
            async move {
                if let Some(project) = remote_project {
                    vorkspace_core::catalog::remote_models(&project, kind.id())
                } else {
                    catalog::models_for(kind, &account)
                }
            }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(models) = work.await {
                let _ = this.update(cx, |this, cx| {
                    if Kind::from_id(&this.summary.provider) == kind && this.catalog_epoch == epoch
                    {
                        this.catalog = models;
                        if this.is_remote() && this.model.is_none() {
                            this.model = this
                                .catalog
                                .iter()
                                .find(|m| m.default)
                                .or_else(|| this.catalog.first())
                                .map(|m| m.id.clone());
                            this.settings.model = this.model.clone();
                            this.save_settings(cx);
                        }
                        cx.emit(SessionChanged);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
    fn model_info(&self) -> Option<&Model> {
        match self.model.as_deref().or(self.resolved_model.as_deref()) {
            Some(id) => self.catalog.iter().find(|m| m.id == id),
            None => self
                .catalog
                .iter()
                .find(|m| m.default)
                .or_else(|| self.catalog.first()),
        }
    }
    /// A new tab shows the model and access it was created with from its
    /// first frame instead of flipping when the stored settings load.
    pub(crate) fn seed_new_settings(&mut self, settings: Settings, cx: &mut Context<Self>) {
        if self.loaded || self.settings.revision != 0 {
            return;
        }
        if settings.model.is_some() {
            self.model = settings.model.clone();
        }
        self.settings = settings;
        cx.notify();
    }
    fn save_settings(&mut self, cx: &mut Context<Self>) {
        if self.history_partial {
            self.defer_history_action(|this, cx| this.save_settings(cx), cx);
            return;
        }
        self.settings.revision = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let mut settings = self.settings.clone();
        settings.model = self.model.clone();
        let store = self.store.clone();
        let id = self.summary.id;
        let provider = self.summary.provider.clone();
        let remote = self.is_remote();
        let cancelled = self.settings_write_cancel.clone();
        let title = (!self.summary.has_messages).then(|| self.summary.title.clone());
        cx.background_executor()
            .spawn(async move {
                let saved = locked(&store).and_then(|s| {
                    if cancelled.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    s.configure_agent(id, &provider, &settings)?;
                    if let Some(title) = title
                        && s.agent_settings(id)?
                            .is_some_and(|saved| saved.revision == settings.revision)
                    {
                        s.title(id, &title)?;
                    }
                    Ok(())
                });
                if saved.is_ok() && remote && !cancelled.load(Ordering::Acquire) {
                    let _ = crate::runtime::remote::configure_saved(store, id);
                }
            })
            .detach();
    }
    fn menu_choice(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(choice) = id.strip_prefix("recent-model:") {
            if let Ok((provider, model)) = serde_json::from_str::<(String, Option<String>)>(choice)
                && let Some(kind) = Kind::parse(&provider)
            {
                let picked = ModelPicked(kind, model);
                if self.apply_model_pick(&picked, cx) {
                    remember_model(kind, picked.1, cx);
                }
            }
            return;
        }
        if let Some(id) = id.strip_prefix("usage-account:") {
            if let Some(center) = crate::accounts::get(cx) {
                let kind = Kind::from_id(&self.summary.provider);
                if center.read(cx).book.exists(kind, id) {
                    let profile = center.read(cx).profile(id);
                    self.switch_usage_account(profile, cx);
                }
            }
            return;
        }
        if std::env::var_os("VORKSPACE_NATIVE_TRACE").is_some() {
            eprintln!("native settings session={} action={id}", self.summary.id);
        }
        if let Some(mode) = id.strip_prefix("access:") {
            self.settings.access = AccessMode::from_id(mode);
            remember_access(self.settings.access, cx);
        } else if let Some(effort) = id.strip_prefix("effort:") {
            self.set_effort_choice(effort, cx);
        } else if let Some(tier) = id.strip_prefix("tier:") {
            self.settings.service_tier = (!tier.is_empty()).then(|| tier.to_owned());
            self.settings.model_settings.remove("serviceTier");
        } else if let Some(setting) = id.strip_prefix("model-setting:") {
            if let Ok((key, value)) = serde_json::from_str::<(String, String)>(setting) {
                if is_effort_key(&key)
                    && (!self.is_remote() || matches!(key.as_str(), "effort" | "reasoningEffort"))
                {
                    self.set_effort_choice(&value, cx);
                }
                self.settings.model_settings.insert(key, value);
            }
        } else if let Some(key) = id.strip_prefix("model-toggle:") {
            let current = self
                .settings
                .model_settings
                .get(key)
                .map(String::as_str)
                .or_else(|| {
                    self.model_info()
                        .and_then(|model| model.controls.iter().find(|c| c.id == key))
                        .map(|c| c.current.as_str())
                });
            let value = if current == Some("true") {
                "false"
            } else {
                "true"
            };
            self.settings
                .model_settings
                .insert(key.to_owned(), value.to_owned());
        } else {
            match id {
                "workspace:current" => {
                    self.settings.worktree_removed = false;
                    self.settings.new_worktree = false;
                    self.settings.worktree_cwd = None;
                    self.refresh_git(cx);
                }
                "workspace:new" => {
                    self.settings.worktree_removed = false;
                    self.settings.new_worktree = true;
                    self.settings.worktree_cwd = None;
                    self.settings.worktree_base =
                        self.settings.worktree_base.clone().or(self.branch.clone());
                    self.refresh_git(cx);
                }
                "fast" => self.settings.fast = !self.settings.fast,
                "fast:on" => self.settings.fast = true,
                "fast:off" => self.settings.fast = false,
                "thinking" => {
                    self.settings.thinking = Some(!self.settings.thinking.unwrap_or(false))
                }
                "plan" | "operator" | "orchestrator" | "draft" => {
                    self.toggle_mode(id, cx);
                    return;
                }
                "queue" => {
                    self.bot_ui.queue = !self.bot_ui.queue;
                    cx.notify();
                    return;
                }
                _ => return,
            }
        }
        self.save_settings(cx);
        cx.notify();
    }
    fn set_effort_choice(&mut self, effort: &str, cx: &mut Context<Self>) {
        self.settings
            .model_settings
            .retain(|key, _| !is_effort_key(key));
        self.settings.effort = (!effort.is_empty()).then(|| effort.to_owned());
        let provider = self.summary.provider.clone();
        if cx.has_global::<crate::settings::Preferences>() {
            cx.update_global::<crate::settings::Preferences, _>(|prefs, _| {
                if effort.is_empty() {
                    prefs.provider_efforts.remove(&provider);
                } else {
                    prefs.provider_efforts.insert(provider, effort.to_owned());
                }
            });
        }
    }
    fn selected_effort(&self) -> Option<&str> {
        self.settings
            .effort
            .as_deref()
            .or_else(|| {
                [
                    "effort",
                    "reasoning",
                    "reasoningEffort",
                    "reasoning_effort",
                    "variant",
                ]
                .into_iter()
                .find_map(|key| self.settings.model_settings.get(key).map(String::as_str))
            })
            .or_else(|| self.model_info().and_then(|m| m.default_effort.as_deref()))
            .or_else(|| {
                self.model_info()
                    .and_then(|m| m.efforts.first().map(String::as_str))
            })
    }
    fn effort_label(&self) -> SharedString {
        effort_name(self.selected_effort().unwrap_or("default")).into()
    }
    fn effort_setting_label(&self) -> &'static str {
        if Kind::from_id(&self.summary.provider) == Kind::Codex {
            "Reasoning"
        } else {
            "Effort"
        }
    }
    fn selected_tier(&self) -> Option<&str> {
        self.settings
            .service_tier
            .as_deref()
            .or_else(|| {
                self.settings
                    .model_settings
                    .get("serviceTier")
                    .map(String::as_str)
            })
            .filter(|value| !value.is_empty() && *value != "default")
    }
    /// Combined ModelPicker trigger; beside mode renders these as separate pills.
    fn trigger_effort_label(&self) -> Option<SharedString> {
        if let Some(model) = self.model_info() {
            if let Some(control) = model.controls.iter().find(|control| {
                !model_control_toggle(control)
                    && matches!(
                        control.id.as_str(),
                        "effort"
                            | "reasoning"
                            | "reasoningEffort"
                            | "reasoning_effort"
                            | "thinking"
                            | "variant"
                    )
            }) {
                let selected = self
                    .settings
                    .model_settings
                    .get(&control.id)
                    .unwrap_or(&control.current);
                return Some(
                    control
                        .choices
                        .iter()
                        .find(|(value, _)| value == selected)
                        .map(|(_, label)| label)
                        .unwrap_or(selected)
                        .clone()
                        .into(),
                );
            }
            return (!model.efforts.is_empty()).then(|| self.effort_label());
        }
        // Keep the saved effort visible while the live catalog is loading.
        self.settings.effort.as_ref().map(|_| self.effort_label())
    }
    fn effort_items(&self) -> Vec<MenuAction> {
        self.model_info().map_or_else(Vec::new, |model| {
            model
                .efforts
                .iter()
                .map(|effort| {
                    MenuAction::new(format!("effort:{effort}"), effort_name(effort))
                        .checked(self.selected_effort() == Some(effort.as_str()))
                })
                .collect()
        })
    }
    /// The pill's label, which drops the catalog names' "Claude " prefix.
    fn model_label(&self) -> SharedString {
        let name = self
            .model_info()
            .map(|m| model_display_name(Kind::from_id(&self.summary.provider), &m.id, &m.name))
            .unwrap_or_else(|| {
                model_name(
                    Kind::from_id(&self.summary.provider),
                    self.model.as_deref().or(self.resolved_model.as_deref()),
                )
            });
        if Kind::from_id(&self.summary.provider) == Kind::Claude {
            name.strip_prefix("Claude ")
                .map_or(name.clone(), |short| short.to_owned().into())
        } else {
            name
        }
    }
    /// A different provider is armed for the next submission. The running
    /// provider retains its controls, account and immutable turn identity.
    pub(super) fn apply_model_pick(
        &mut self,
        picked: &ModelPicked,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.editing_last_turn.pending() || self.status == "Preparing handoff…" {
            return false;
        }
        let changed_model = self.summary.provider != picked.0.id() || self.model != picked.1;
        if std::env::var_os("VORKSPACE_NATIVE_TRACE").is_some() {
            eprintln!(
                "native model session={} provider={} model={:?}",
                self.summary.id,
                picked.0.id(),
                picked.1
            );
        }
        if self.summary.provider != picked.0.id() {
            let reverting = self
                .settings
                .pending_provider_switch
                .as_ref()
                .is_some_and(|pending| pending.from == picked.0.id());
            let restored = reverting
                .then(|| self.settings.pending_provider_switch.clone())
                .flatten();
            let changed = locked(&self.store).and_then(|store| {
                store.change_provider(self.summary.id, picked.0.id())?;
                if let Some(id) = restored
                    .as_ref()
                    .and_then(|pending| pending.provider_id.as_deref())
                {
                    store.provider_id(self.summary.id, id)?;
                }
                Ok(())
            });
            if let Err(error) = changed {
                self.status = error;
                cx.notify();
                return false;
            }
            if let Some(pending) = restored {
                self.settings.account = pending.account;
                self.summary.provider_id = pending.provider_id;
                self.settings.pending_provider_switch = None;
                self.settings.handoff = None;
            } else {
                let has_history = self.busy
                    || self
                        .transcript
                        .read(cx)
                        .source_rows()
                        .iter()
                        .any(|row| row.is_prompt());
                if has_history && self.settings.pending_provider_switch.is_none() {
                    self.settings.pending_provider_switch =
                        Some(vorkspace_core::agent::ProviderSwitch {
                            from: self.summary.provider.clone(),
                            cwd: self.execution_cwd().to_owned(),
                            model: self.model.clone(),
                            model_settings: self.settings.model_settings.clone(),
                            effort: self.settings.effort.clone(),
                            fast: self.settings.fast,
                            service_tier: self.settings.service_tier.clone(),
                            thinking: self.settings.thinking,
                            skip_outgoing_recap: self.settings.usage_limit.is_some(),
                            account: self.settings.account.clone(),
                            provider_id: self.summary.provider_id.clone(),
                        });
                }
                self.settings.account = crate::accounts::get(cx)
                    .map(|center| center.read(cx).selected(picked.0, &self.summary.cwd))
                    .unwrap_or_default();
                self.summary.provider_id = None;
                if let Some(pending) = &self.settings.pending_provider_switch {
                    self.settings.handoff = Some(vorkspace_core::agent::Handoff {
                        from: pending.from.clone(),
                        brief: vorkspace_core::response::handoff(
                            self.transcript.read(cx).source_rows(),
                        ),
                    });
                }
            }
            self.summary.provider = picked.0.id().into();
            self.completion.update(cx, |completion, cx| {
                completion.set_provider(picked.0.id(), cx)
            });
            self.catalog.clear();
            if !self.summary.has_messages {
                self.summary.title = format!("New {} session", picked.0.label());
            }
        }
        self.model = picked.1.clone();
        if let Some(context) = &mut self.context {
            context.window = None;
        }
        let store = self.store.clone();
        let id = self.summary.id;
        let context = self.context;
        cx.background_executor()
            .spawn(async move {
                if let Some(context) = context {
                    let _ = locked(&store).and_then(|s| s.save_context_usage(id, &context));
                }
            })
            .detach();
        self.resolved_model = None;
        self.settings.effort = preferred_effort(&self.summary.provider, cx);
        self.settings.fast = false;
        self.settings.thinking = None;
        self.settings.service_tier = None;
        self.settings.model_settings.clear();
        if changed_model && self.settings.usage_limit.is_some() && !self.busy {
            self.recover_usage_after_model_change(cx);
        }
        self.refresh_catalog(cx);
        self.save_settings(cx);
        cx.emit(SessionChanged);
        true
    }
    fn menu_is(&self, kind: ComposerMenu) -> bool {
        self.menu.as_ref().is_some_and(|open| open.kind == kind)
    }
    fn anchor(&self, kind: ComposerMenu) -> &AnchorSlot {
        match kind {
            ComposerMenu::Add => &self.add_anchor,
            ComposerMenu::Model | ComposerMenu::Recent => &self.model_anchor,
            ComposerMenu::Access => &self.access_anchor,
            ComposerMenu::Account => &self.account_anchor,
            ComposerMenu::Workspace => &self.workspace_anchor,
            ComposerMenu::Setting(ix) => self
                .bot_ui
                .setting_anchors
                .get(ix)
                .unwrap_or(&self.model_anchor),
        }
    }
    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }
    fn toggle_menu(&mut self, kind: ComposerMenu, window: &mut Window, cx: &mut Context<Self>) {
        if kind == ComposerMenu::Model
            && cx
                .try_global::<crate::settings::Preferences>()
                .is_some_and(|p| p.model_controls == "beside")
        {
            self.toggle_bot_models(window, cx);
            return;
        }
        self.bot_ui.models = None;
        if self.menu.as_ref().is_some_and(|open| open.kind == kind) {
            self.close_menu(window, cx);
            return;
        }
        // Another menu gives way at once rather than overlapping this one.
        self.menu = None;
        self.completion
            .update(cx, |completion, cx| completion.dismiss(cx));
        self.git_picker = None;
        self.git_events = None;
        let mut events = Vec::new();
        let (label, items, width, style) = self.menu_items(kind, &mut events, window, cx);
        // The recent models open on the current one.
        let current = (kind == ComposerMenu::Recent)
            .then(|| {
                items.iter().find_map(|item| match item {
                    MenuItem::Action(action) if action.checked => Some(action.id.clone()),
                    _ => None,
                })
            })
            .flatten();
        let menu = cx.new(|cx| {
            let menu = Menu::new(label, items, window, cx)
                .width(width)
                .style(style);
            match current {
                Some(id) => menu.select_value(&id),
                None => menu,
            }
        });
        events.push(
            cx.subscribe_in(&menu, window, |this, menu, event: &MenuEvent, _, cx| {
                if let MenuEvent::Picked(id, _) = event {
                    let toggle = menu.read(cx).toggle_state(id);
                    if id.as_ref() == "upload" {
                        this.choose_attachments(cx);
                    } else {
                        this.menu_choice(id, cx);
                    }
                    if let Some(on) = toggle {
                        menu.update(cx, |menu, cx| menu.set_toggle(id, !on, cx));
                        cx.notify();
                        return;
                    }
                }
                this.menu = None;
                cx.notify();
            }),
        );
        self.menu = Some(OpenMenu {
            kind,
            menu,
            _events: events,
        });
        cx.notify();
    }
    fn menu_items(
        &self,
        kind: ComposerMenu,
        events: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (&'static str, Vec<MenuItem>, f32, MenuStyle) {
        let provider = Kind::from_id(&self.summary.provider);
        match kind {
            ComposerMenu::Account => {
                let items = crate::accounts::get(cx)
                    .map(|center| {
                        center
                            .read(cx)
                            .book
                            .list(provider)
                            .into_iter()
                            .map(|account| {
                                MenuAction::new(
                                    format!("usage-account:{}", account.id),
                                    account.label,
                                )
                                .checked(account.id == self.settings.account.id)
                                .into()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                ("Switch account", items, 250., MenuStyle::Explorer)
            }
            ComposerMenu::Workspace => {
                let picker = cx.new(|cx| {
                    GitPicker::new(
                        self.summary.cwd.clone().into(),
                        GitMode::Worktree,
                        Some(self.execution_cwd().to_owned()),
                        cx,
                    )
                    .existing_worktrees()
                });
                events.push(cx.subscribe_in(
                    &picker,
                    window,
                    |this, _, picked: &GitPicked, window, cx| match picked {
                        GitPicked::Worktree(path) => {
                            if this.use_existing_worktree(path, cx) {
                                this.close_menu(window, cx);
                            }
                        }
                        GitPicked::Dismissed => this.close_menu(window, cx),
                        _ => {}
                    },
                ));
                let focus = picker.read(cx).focus(cx);
                (
                    "Workspace",
                    vec![
                        MenuItem::Header("Workspace".into()),
                        MenuAction::new("workspace:current", "Current checkout")
                            .icon("folder")
                            .checked(
                                !self.settings.new_worktree && self.settings.worktree_cwd.is_none(),
                            )
                            .into(),
                        MenuAction::new("workspace:new", "New worktree")
                            .icon("folder-tree")
                            .checked(self.settings.new_worktree)
                            .into(),
                        MenuItem::Separator,
                        MenuAction::new("workspace:existing", "Existing worktrees")
                            .icon("folder-tree")
                            .flyout(Flyout {
                                view: picker.into(),
                                width: 300.,
                                padding: 0.,
                                height: None,
                                max_height: Some(320.),
                                focus: Some(focus),
                                surface: None,
                            })
                            .into(),
                    ],
                    240.,
                    MenuStyle::Settings,
                )
            }
            // Composer.tsx's plus menu with native mode and attachment actions.
            ComposerMenu::Add => {
                let mut items = vec![MenuItem::Header("Add to message".into())];
                items.extend(
                    [
                        MenuAction::new("upload", "Upload file")
                            .icon("file-add")
                            .description("Attach files or images"),
                        MenuAction::new("plan", "Plan mode")
                            .icon("ai-idea")
                            .icon_color(tint(YELLOW_300, 0.8))
                            .description("Review a plan before building"),
                        MenuAction::new("operator", "Operator")
                            .disabled(self.is_remote())
                            .icon("cursor-magic")
                            .icon_color(tint(SKY_300, 0.8))
                            .description("Give this thread access to vorkspace"),
                        MenuAction::new("orchestrator", "Orchestrator")
                            .disabled(self.is_remote())
                            .icon("share")
                            .icon_color(tint(FUCHSIA_300, 0.65))
                            .badge("v1")
                            .description("Plan and coordinate agent work"),
                        MenuAction::new("draft", "Draft")
                            .icon("circle-dashed-hi")
                            .description("Save this message without starting the agent")
                            .disabled(self.busy || self.follow_ups.state.draft.is_some()),
                    ]
                    .into_iter()
                    .map(|action| {
                        let checked = match action.id.as_ref() {
                            "plan" => self.settings.plan,
                            "draft" => self.draft_active(cx),
                            "operator" => self.settings.operator,
                            "orchestrator" => self.settings.orchestrator,
                            _ => false,
                        };
                        action.checked(checked).into()
                    }),
                );
                if self.settings.bot.is_some() {
                    items.push(
                        MenuAction::new("queue", "Queue")
                            .icon("queue-list")
                            .description("Send after the current reply instead of steering it")
                            .checked(self.bot_ui.queue)
                            .into(),
                    );
                }
                ("Add to message", items, 250., MenuStyle::Composer)
            }
            // ModelPicker: a "Model" row whose flyout lists the models.
            ComposerMenu::Model => {
                let list = cx.new(|cx| {
                    let hidden = cx
                        .try_global::<crate::settings::Preferences>()
                        .map(|p| p.hidden_providers.clone())
                        .unwrap_or_default();
                    ModelList::new_for(
                        provider,
                        self.model.clone(),
                        &self.summary.cwd,
                        self.settings.account.clone(),
                        cx,
                    )
                    .hide_providers(hidden)
                    .allow_provider_switch(true)
                });
                events.push(cx.subscribe_in(
                    &list,
                    window,
                    |this, _, picked: &ModelPicked, window, cx| {
                        if this.apply_model_pick(picked, cx) {
                            this.close_menu(window, cx);
                        }
                    },
                ));
                let focus = list.read(cx).search_focus(cx);
                let mut items = if cx
                    .try_global::<crate::settings::Preferences>()
                    .is_none_or(|p| p.model_controls != "beside")
                {
                    self.bot_model_options()
                } else {
                    Vec::new()
                };
                items.push(
                    MenuAction::new("model", "Model")
                        .value_image(provider.id())
                        .value(self.model_info().map_or_else(
                            || model_name(provider, self.model.as_deref()),
                            |model| model_display_name(provider, &model.id, &model.name),
                        ))
                        .flyout(Flyout {
                            view: list.into(),
                            width: 310.,
                            padding: 0.,
                            height: Some(442.),
                            max_height: None,
                            focus: Some(focus),
                            surface: None,
                        })
                        .into(),
                );
                ("Model and settings", items, 250., MenuStyle::Settings)
            }
            ComposerMenu::Recent => (
                "Recently used models",
                self.recent_model_items(cx),
                250.,
                MenuStyle::Recent,
            ),
            ComposerMenu::Access => {
                let items = self.bot_access_options();
                ("Access", items, 288., MenuStyle::Access)
            }
            ComposerMenu::Setting(ix) => self.bot_setting_menu(ix),
        }
    }
    /// The model trigger: the composer's model menu.
    #[cfg(test)]
    pub(crate) fn toggle_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.bot.is_some() {
            self.toggle_bot_models(window, cx);
            return;
        }
        self.toggle_menu(ComposerMenu::Model, window, cx);
    }
    /// `App: Switch Model` (Cmd+.): toggles the recent models, as ModelPicker's
    /// hotkey does.
    pub(crate) fn toggle_recent_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bot_ui.models = None;
        self.toggle_menu(ComposerMenu::Recent, window, cx);
    }
    /// Right-clicking the model trigger opens the recent models over any menu.
    fn open_recent_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.menu_is(ComposerMenu::Recent) {
            self.toggle_recent_models(window, cx);
        }
    }
    /// recentMenuModels: up to six remembered choices, with the current model
    /// added when it is not among them.
    fn recent_model_items(&self, cx: &gpui::App) -> Vec<MenuItem> {
        let provider = Kind::from_id(&self.summary.provider);
        let current = (provider, self.model.clone());
        let hidden = cx
            .try_global::<crate::settings::Preferences>()
            .map(|p| p.hidden_providers.clone())
            .unwrap_or_default();
        let mut choices: Vec<(Kind, Option<String>)> = cx
            .try_global::<crate::settings::Preferences>()
            .into_iter()
            .flat_map(|p| &p.recent_models)
            .filter_map(|choice| {
                let kind = Kind::parse(&choice.provider)?;
                choice.model.as_ref()?;
                let selected = (kind, choice.model.clone()) == current;
                let model_hidden = cx
                    .try_global::<crate::settings::Preferences>()
                    .is_some_and(|p| p.model_hidden(kind.id(), choice.model.as_deref().unwrap()));
                ((kind == provider || !hidden.iter().any(|id| id == kind.id()))
                    && (selected || !model_hidden))
                    .then(|| (kind, choice.model.clone()))
            })
            .collect();
        choices.dedup();
        if !choices.iter().take(6).any(|choice| choice == &current) {
            choices.truncate(5);
            choices.push(current.clone());
        }
        choices.truncate(6);
        choices
            .into_iter()
            .map(|(kind, model)| {
                let selected = (kind, model.clone()) == current;
                let name = if selected {
                    self.model_label()
                } else {
                    let name = cached_model_name(kind, model.as_deref());
                    match name.strip_prefix("Claude ") {
                        Some(short) if kind == Kind::Claude => short.to_owned().into(),
                        _ => name,
                    }
                };
                let id = serde_json::to_string(&(kind.id(), &model)).unwrap_or_default();
                MenuAction::new(format!("recent-model:{id}"), name)
                    .image(kind.id())
                    .description(kind.label())
                    .checked(selected)
                    .into()
            })
            .collect()
    }
    /// `Session: Stop` on Escape: cancels a running turn.
    pub(crate) fn stop_if_busy(&mut self, cx: &mut Context<Self>) {
        if self.cancel_edit_last_turn(cx) {
            return;
        }
        if self.busy || self.ticket_context.pending() {
            self.stop(cx);
        }
    }
    fn stop(&mut self, cx: &mut Context<Self>) {
        let ticket_pending = self.ticket_context.pending();
        self.ticket_context.cancel();
        if ticket_pending {
            self.status = "Ready".into();
            cx.notify();
        }
        if !self.busy || self.stopping {
            return;
        }
        self.stopping = true;
        self.busy = false;
        self.follow_ups.resuming = false;
        self.follow_ups.continuations.clear();
        self.notification_cancelled = true;
        self.cancel.store(true, Ordering::Relaxed);
        if !self.follow_ups.state.messages.is_empty() {
            self.follow_ups.state.paused = true;
            self.follow_ups.requested = None;
            self.save_follow_ups(cx);
        }
        self.follow_ups.ready = false;
        self.approvals.clear();
        self.questions.clear();
        self.controls = None;
        self.status = "Ready".into();
        self.transcript
            .update(cx, |view, cx| view.stop_streaming(cx));
        self.review
            .update(cx, |review, cx| review.set_busy(false, cx));
        cx.emit(SessionChanged);
        cx.notify();
    }
    // Composer.tsx in the inset layout: a frosted, rounded box holding the
    // field and its toolbar, with the workspace bar sitting under it.
    fn composer(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let command = if self.follow_ups.draft_selected {
            Some(input::Command::Draft)
        } else if self.settings.orchestrator {
            Some(input::Command::Orchestrator)
        } else if self.settings.plan {
            Some(input::Command::Plan)
        } else if self.settings.operator {
            Some(input::Command::Operator)
        } else {
            None
        };
        self.input
            .update(cx, |input, cx| input.selected_command(command, cx));
        self.completion.update(cx, |completion, cx| {
            completion.set_cwd(
                self.settings
                    .worktree_cwd
                    .as_deref()
                    .unwrap_or(&self.summary.cwd),
                cx,
            )
        });
        let menu: Vec<_> = self
            .menu
            .as_ref()
            .map(|open| {
                menu_popover(&open.menu, Anchor::Slot(self.anchor(open.kind).clone()), cx)
                    .side(Side::Top)
                    .when(open.kind == ComposerMenu::Account, |p| {
                        p.width(250.).max_height(260.)
                    })
            })
            .into_iter()
            .collect();
        div()
            .relative()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(self.composer_anchor.probe())
            .children(menu)
            .children(self.model_list_popover(cx))
            .children(self.orchestration_card(cx))
            .children(if self.is_remote() { None } else { self.usage_notice(false, cx) })
            .child(self.message_queue(window, cx))
            .children(if self.is_remote() { None } else { self.auth_notice(cx) })
            .when(self.settings.worktree_removed,|d|d.child(div().mb_2().p_3().rounded_lg().border_1().border_color(content(0.1)).flex().items_center().gap_3().text_size(px(12.)).child(div().flex_1().child("Worktree deleted. Select a branch or worktree to continue this conversation.")).child(tool_pill("reattach-worktree","Select worktree").child("Select worktree").on_click(cx.listener(|this,_,window,cx|this.open_git(GitMode::Worktree,window,cx))))))
            .when_some(self.settings.handoff.clone(), |d, handoff| {
                d.child(
                    div()
                        .mb(px(6.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(10.))
                        .bg(content(0.05))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .text_color(content(0.7))
                        .child(icon("response-handoff", 14.))
                        .child(div().min_w_0().flex_1().child(format!(
                            "Handoff from {} · Add your next instruction below",
                            Kind::display_label(&handoff.from)
                        )))
                        .child(
                            tool_pill("dismiss-handoff", "Dismiss handoff")
                                .child(icon("close", 12.))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.settings.handoff = None;
                                    this.save_settings(cx);
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.git_picker.clone(), |d, picker| {
                let anchor = if picker.read(cx).mode() == GitMode::Worktree {
                    self.workspace_anchor.clone()
                } else {
                    self.branch_anchor.clone()
                };
                let weak = cx.entity().downgrade();
                d.child(
                    popover("composer-git-picker", Anchor::Slot(anchor))
                        .side(Side::Top)
                        .width(300.)
                        .height(320.)
                        .on_dismiss(move |window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.git_picker = None;
                                this.git_events = None;
                                window.focus(&this.input.focus_handle(cx), cx);
                                cx.notify();
                            });
                        })
                        .child(picker),
                )
            })
            .child(self.approval_shelf(window, cx))
            .child(self.composer_box(window, cx))
            .children(self.remote_notice(cx))
            .child(self.composer_tools(cx))
            .into_any_element()
    }
    /// The composer box shared by sessions and Bots: its field, with send
    /// beside it, on the frosted composer surface.
    pub(super) fn composer_box(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        const RADIUS: f32 = 8.;
        let blur = composer_blur();
        let fill = composer_fill();
        let bot = self.settings.bot.is_some();
        let input = self.input.read(cx);
        let has_value = input.has_message_text() || !self.attachments.is_empty();
        // Sessions complete files, commands and skills; a Bot's field is plain.
        let field = if bot {
            self.input.clone().into_any_element()
        } else {
            self.completion.clone().into_any_element()
        };
        div()
            .id("composer-box")
            .debug_selector(|| "composer-box".into())
            .capture_action(cx.listener(|this, _: &input::Paste, _, cx| {
                if this.paste_attachments(cx) {
                    cx.stop_propagation();
                }
            }))
            .relative()
            .rounded(px(RADIUS))
            .border_1()
            .border_color(composer_border(
                self.input.focus_handle(cx).contains_focused(window, cx),
            ))
            .when(self.file_drag, |d| {
                d.border_color(accent_color().opacity(0.6))
            })
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.input.focus_handle(cx), cx)),
            )
            .when(!bot, |d| d.child(self.completion.read(cx).composer_probe()))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let surface = gpui::fill(bounds, fill).corner_radii(px(RADIUS - 1.));
                        let surface = if blur > 0. {
                            surface.border_color(gpui_apple::metal_renderer::backdrop_blur_marker(
                                blur * window.scale_factor(),
                            ))
                        } else {
                            surface
                        };
                        window.paint_quad(surface);
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .children(self.composer_recovery_banner(cx))
            .children(self.local_submission_banner(cx))
            .children(self.attachment_chips(cx))
            .child(
                // Even padding centres send beside one line and keeps it at
                // the bottom as the text grows.
                div()
                    .flex()
                    .items_end()
                    .gap_1p5()
                    .p(px(5.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .self_center()
                            .px_1p5()
                            .py_1()
                            .child(field),
                    )
                    .child(self.action(has_value, cx)),
            )
            .children(self.attachment_drop_overlay(RADIUS))
            .into_any_element()
    }

    /// The shared `+`: files and modes for a session, files and queueing for
    /// a Bot.
    pub(super) fn add_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.menu_is(ComposerMenu::Add);
        quiet_control("composer-add", open)
            .debug_selector(|| "composer-add".into())
            .group("composer-add")
            .w(px(26.))
            .px_0()
            .justify_center()
            .aria_label(if self.settings.bot.is_some() {
                "Upload a file or queue this message"
            } else {
                "Add files or choose a mode"
            })
            .aria_expanded(open)
            .child(self.add_anchor.probe())
            .child(
                icon("plus", 14.)
                    .text_color(content(if open { 1. } else { 0.55 }))
                    .group_hover("composer-add", |s| s.text_color(content(1.))),
            )
            .on_click(
                cx.listener(|this, _, window, cx| this.toggle_menu(ComposerMenu::Add, window, cx)),
            )
    }
    /// The row under the box, shared by sessions and Bots: `+` (and for a
    /// session its working copy and branch) on the left; the model, its
    /// settings, access and context on the right.
    pub(super) fn composer_tools(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let bot = self.settings.bot.is_some();
        let kind = Kind::from_id(&self.summary.provider);
        let beside = cx
            .try_global::<crate::settings::Preferences>()
            .is_some_and(|p| p.model_controls == "beside");
        let trigger_effort = (!beside).then(|| self.trigger_effort_label()).flatten();
        let controls = if beside {
            self.model_controls(cx)
        } else {
            Vec::new()
        };
        let draft = !self.summary.has_messages;
        let linked =
            self.git.as_ref().is_some_and(|s| s.linked) || self.settings.worktree_cwd.is_some();
        let new = draft && self.settings.new_worktree;
        let label = if new {
            "New worktree"
        } else if linked {
            "Worktree"
        } else {
            "Current checkout"
        };
        let enabled = !self.busy && !self.preparing_worktree && self.git.is_some();
        // Still reading git is not unavailable: don't dim, then brighten.
        let dimmed = !enabled && (self.loaded || self.busy || self.preparing_worktree);
        let workspace = if draft {
            bar_trigger(
                "composer-workspace",
                if new || linked {
                    "folder-tree"
                } else {
                    "folder"
                },
                label.into(),
            )
            .debug_selector(|| "composer-workspace".into())
            .aria_label(format!("Workspace {label}"))
            .aria_expanded(self.menu_is(ComposerMenu::Workspace))
            .child(self.workspace_anchor.probe())
            .when(dimmed, |d| d.opacity(0.4))
            .on_click(cx.listener(move |this, _, window, cx| {
                if enabled {
                    this.refresh_git(cx);
                    this.toggle_menu(ComposerMenu::Workspace, window, cx);
                }
            }))
            .into_any_element()
        } else {
            // WorkspaceIdentity: a started conversation owns this working copy.
            div()
                .id("composer-workspace")
                .debug_selector(|| "composer-workspace".into())
                .role(Role::Group)
                .aria_label(format!("Workspace {label}"))
                .flex_shrink_0()
                .h(px(26.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(content(0.55 * 0.4))
                .child(
                    icon(if linked { "folder-tree" } else { "folder" }, 14.)
                        .text_color(content(0.55 * 0.4)),
                )
                .child(label)
                .into_any_element()
        };
        let branch = if new {
            self.settings
                .worktree_base
                .as_deref()
                .map(crate::git_picker::base_label)
                .map(str::to_owned)
                .or(self.branch.clone())
        } else {
            self.branch.clone()
        };
        let ratio = self.context.and_then(|usage| usage.ratio());
        let show_context = bot || ratio.is_some() || draft;
        let headline = ratio
            .map(|r| format!("{:.0}% context used", r * 100.))
            .unwrap_or_else(|| "No context used yet".into());
        let detail = self.context.map(|usage| {
            format!(
                "{} / {} tokens",
                vorkspace_core::context::format_tokens(usage.used),
                vorkspace_core::context::format_tokens(usage.window.unwrap_or(0))
            )
        });
        let model_open = self.menu_is(ComposerMenu::Model)
            || self.menu_is(ComposerMenu::Recent)
            || self.bot_ui.models.is_some();
        let model =
            quiet_control("composer-model", model_open)
                .debug_selector(|| "composer-model".into())
                .min_w_0()
                .max_w(px(220.))
                .flex_shrink(1.)
                .aria_label(format!(
                    "{}, {}{}",
                    kind.label(),
                    self.model_label(),
                    trigger_effort
                        .as_ref()
                        .map(|effort| format!(", effort {effort}"))
                        .unwrap_or_default()
                ))
                .child(self.model_anchor.probe())
                .aria_expanded(model_open)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_menu(ComposerMenu::Model, window, cx)
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_recent_models(window, cx)
                    }),
                )
                .child(provider_icon(&self.summary.provider, 14.))
                .child(
                    div()
                        .debug_selector(|| "composer-model-label".into())
                        .min_w_0()
                        .truncate()
                        .child(self.model_label()),
                )
                .when_some(trigger_effort, |d, effort| {
                    d.child(
                        div()
                            .debug_selector(|| "composer-model-effort".into())
                            .min_w_0()
                            .flex_shrink(1.)
                            .truncate()
                            .text_size(px(11.))
                            .text_color(content(0.5))
                            .child(effort),
                    )
                });
        let access_open = self.menu_is(ComposerMenu::Access);
        let access = quiet_control("composer-access", access_open)
            .debug_selector(|| "composer-access".into())
            .aria_label(format!("Access, {}", self.settings.access.label()))
            .aria_expanded(access_open)
            .child(self.access_anchor.probe())
            .on_click(
                cx.listener(|this, _, window, cx| {
                    this.toggle_menu(ComposerMenu::Access, window, cx)
                }),
            )
            .child(
                icon(
                    match self.settings.access {
                        AccessMode::FullAccess => "shield-alert",
                        AccessMode::AutoAcceptEdits => "pencil-edit",
                        AccessMode::Auto => "sparkles",
                        _ => "square-lock",
                    },
                    14.,
                )
                .text_color(if self.settings.access == AccessMode::FullAccess {
                    status(StatusColor::Warning, 1.)
                } else {
                    content(if access_open { 1. } else { 0.55 })
                }),
            )
            .child(div().truncate().child(self.settings.access.label()));
        div()
            .debug_selector(|| "composer-toolbar".into())
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(2.))
            .pt_1()
            .pl_1()
            .pr_1p5()
            .child(self.add_button(cx))
            .when(bot && self.bot_ui.queue, |d| {
                d.child(
                    quiet_control("composer-queue-mode", true)
                        .debug_selector(|| "composer-queue-mode".into())
                        .aria_label("Queue is on; click to send normally")
                        .child(icon("queue-list", 14.))
                        .child("Queue")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.bot_ui.queue = false;
                            cx.notify();
                        })),
                )
            })
            // A Bot works across its projects; a session owns one checkout.
            .when(!bot, |d| d.child(workspace))
            .when_some(branch.filter(|_| !bot), |d, branch| {
                let detached = !new && self.git.as_ref().is_some_and(|s| s.detached);
                let label = if detached {
                    format!("detached {branch}")
                } else {
                    branch
                };
                d.child(
                    bar_trigger("composer-branch", "git-branch", label.clone().into())
                        .debug_selector(|| "composer-branch".into())
                        .aria_label(format!(
                            "{} {label}",
                            if new { "Worktree base" } else { "Branch" }
                        ))
                        .aria_description(self.execution_cwd().to_owned())
                        .max_w(px(256.))
                        .child(self.branch_anchor.probe())
                        .when(dimmed, |d| d.opacity(0.4))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if enabled {
                                this.open_git(
                                    if new { GitMode::Base } else { GitMode::Branch },
                                    window,
                                    cx,
                                );
                            }
                        })),
                )
            })
            .child(
                div()
                    .ml_auto()
                    .pl_2()
                    .min_w_0()
                    .max_w_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(model)
                    .children(controls)
                    .when(!bot, |d| d.child(access))
                    .when(self.editing_last_turn.pending(), |d| {
                        d.child(
                            div()
                                .id("cancel-edit-last-turn")
                                .role(Role::Button)
                                .aria_label("Cancel editing")
                                .text_size(px(11.))
                                .text_color(content(0.6))
                                .child("Cancel edit")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel_edit_last_turn(cx);
                                })),
                        )
                    })
                    .when(
                        !bot && self.status != "Ready"
                            && !self.busy
                            && !self.loading
                            && self.settings.usage_limit.is_none(),
                        |d| {
                            d.child(
                                div()
                                    .id("provider-status")
                                    .debug_selector(|| "provider-status".into())
                                    .role(Role::Status)
                                    .aria_label(self.status.clone())
                                    .max_w(px(220.))
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(content(0.45))
                                    .child(self.status.clone()),
                            )
                        },
                    )
                    .when(show_context, |d| {
                        d.child(
                            div()
                                .id("context-meter")
                                .debug_selector(|| "context-meter".into())
                                .role(Role::Image)
                                .aria_label(format!(
                                    "{headline}{}",
                                    detail
                                        .as_ref()
                                        .map(|s| format!(", {s}"))
                                        .unwrap_or_default()
                                ))
                                .child(self.context_anchor.probe())
                                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                    this.context_hovered = *hovered;
                                    cx.notify();
                                }))
                                .child(crate::context_meter::ring(ratio.unwrap_or(0.)))
                                .when(self.context_hovered, |d| {
                                    d.child(
                                        popover(
                                            "context-tooltip",
                                            Anchor::Slot(self.context_anchor.clone()),
                                        )
                                        .side(Side::Top)
                                        .align(Align::End)
                                        .max_width(240.)
                                        .padding(10.)
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .text_size(px(12.))
                                                .line_height(px(16.))
                                                .text_color(content(1.))
                                                .child(headline)
                                                .when_some(detail, |d, detail| {
                                                    d.child(
                                                        div()
                                                            .text_size(px(11.))
                                                            .text_color(content(0.5))
                                                            .child(detail),
                                                    )
                                                }),
                                        ),
                                    )
                                }),
                        )
                    }),
            )
            .into_any_element()
    }

    // ComposerAction: a solid submit, dimmed while empty; Stop while busy.
    fn action(&self, has_value: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let ticket_pending = self.ticket_context.pending();
        let bot = self.settings.bot.is_some();
        let busy = !bot && (ticket_pending || (self.responding() && !has_value));
        let draft = self.draft_active(cx);
        let enabled = ticket_pending
            || (!self.attaching
                && (!self.preparing_worktree || self.busy || self.stopping)
                && (busy || (has_value && self.loaded))
                && (busy || !draft || (!self.busy && self.follow_ups.state.draft.is_none())));
        let accent = primary_action(enabled);
        div()
            .id("send-or-stop")
            .debug_selector(|| "send-or-stop".into())
            .role(Role::Button)
            .aria_label(if busy {
                "Stop"
            } else if draft {
                "Save draft"
            } else {
                "Send"
            })
            .cursor(CursorStyle::Arrow)
            .flex_shrink_0()
            .size(px(26.))
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match accent {
                Some((fill, _)) => d
                    .bg(fill)
                    .when(enabled, |d| d.hover(|s| s.bg(fill.blend(content(0.12))))),
                None => d
                    .bg(white(if enabled { 1. } else { 0.3 }))
                    .when(enabled, |d| d.hover(|s| s.bg(white(0.9)))),
            })
            .map(|d| {
                let ink = accent.map_or(black(if enabled { 1. } else { 0.4 }), |(_, ink)| ink);
                if busy {
                    d.child(div().size(px(10.)).rounded(px(2.)).bg(ink))
                } else {
                    d.child(icon("send", 14.).text_color(ink))
                }
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                if !enabled {
                    return;
                }
                if busy {
                    this.stop(cx);
                } else {
                    this.send(cx);
                }
            }))
    }
    // EmptySession.tsx: the composer centred under its question, or resting
    // where it will dock once a neighbouring pane squeezes it narrower than
    // the column. A narrow window alone keeps it centred.
    fn empty(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let narrow = self.narrow;
        let docked = narrow && self.beside_pane;
        self.attachment_drop_target("empty-session", cx)
            .relative()
            .size_full()
            .flex()
            .justify_center()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let now = bounds.size.width < px(COMPOSER_COLUMN);
                        if now != narrow {
                            cx.defer(move |cx| {
                                let _ = this.update(cx, |this, cx| {
                                    this.narrow = now;
                                    cx.notify();
                                });
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(COMPOSER_COLUMN))
                    .flex()
                    .flex_col()
                    .px(px(10.))
                    .pt(px(24.))
                    .pb(px(6.))
                    .child(div().flex_grow(1.).flex_basis(px(0.)))
                    .when(!docked, |d| {
                        d.child(
                            div()
                                .id("empty-session-title")
                                .role(Role::Heading)
                                .flex_shrink_0()
                                .mb_4()
                                .px(px(10.))
                                .text_size(px(18.))
                                .line_height(px(28.))
                                .text_color(content(1.))
                                .truncate()
                                .child(format!(
                                    "What should we work on in {}?",
                                    self.project_appearance.label(&self.summary.cwd)
                                )),
                        )
                    })
                    .child(self.composer(window, cx))
                    .when(!docked, |d| {
                        d.child(div().flex_grow(1.).flex_shrink(1.).flex_basis(px(18.)))
                    }),
            )
            .into_any()
    }
}

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
mod composer_motion_tests {
    use super::*;
    use gpui::{
        Bounds, EntityInputHandler, TestAppContext, WindowBounds, WindowOptions, point, size,
    };
    use std::{sync::Mutex, time::Duration};
    use vorkspace_core::store::Store;

    fn window(cx: &mut TestAppContext, width: f32) -> gpui::WindowHandle<Conversation> {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("codex", "/work/vorkspace").unwrap();
        let store = Arc::new(Mutex::new(store));
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(width), px(700.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| Conversation::new(summary, store, cx)),
            )
            .unwrap()
        });
        window
            .update(cx, |c, w, cx| {
                c.loaded = true;
                c.status = "Ready".into();
                w.focus(&c.input.focus_handle(cx), cx);
                c.input.update(cx, |i, cx| {
                    i.replace_text_in_range(None, "First message", w, cx)
                });
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        window
    }
    fn begin(
        window: gpui::WindowHandle<Conversation>,
        cx: &mut TestAppContext,
    ) -> Bounds<gpui::Pixels> {
        let from = window
            .update(cx, |c, _, cx| {
                let from = c.composer_anchor.get().unwrap();
                // Use the first-send presentation path without a provider request.
                c.input.update(cx, |i, cx| {
                    i.take(cx);
                });
                c.begin_turn(cx);
                cx.notify();
                from
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        from
    }
    #[gpui::test]
    fn composer_toolbar_wraps_without_overlapping_and_keeps_menus_clickable(
        cx: &mut TestAppContext,
    ) {
        crate::theme::configure(&Default::default(), false, true);
        cx.update(|cx| {
            cx.set_global(crate::settings::Preferences::default());
            crate::clock::use_test_clock(cx);
        });
        let window = window(cx, 760.);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        for width in [760., 480., 360., 260., 760.] {
            visual.simulate_resize(size(px(width), px(700.)));
            for started in [false, true] {
                for beside in [false, true] {
                    window
                        .update(visual, |view, _, cx| {
                            cx.global_mut::<crate::settings::Preferences>()
                                .model_controls = if beside { "beside" } else { "combined" }.into();
                            view.summary.has_messages = started;
                            view.branch = Some("feature/a-very-long-branch-name".into());
                            view.model = Some("gpt-6.1-sol".into());
                            view.settings.effort = Some("xhigh".into());
                            view.settings.access = AccessMode::FullAccess;
                            view.catalog = vec![Model {
                                alias_target: None,
                                id: "gpt-6.1-sol".into(),
                                name: "GPT-6.1 Sol".into(),
                                efforts: vec!["high".into(), "xhigh".into()],
                                default_effort: Some("high".into()),
                                fast: false,
                                thinking: false,
                                default: true,
                                tiers: Vec::new(),
                                controls: Vec::new(),
                            }];
                            cx.notify();
                        })
                        .unwrap();
                    visual.run_until_parked();
                    visual.refresh().unwrap();
                    let toolbar = visual.debug_bounds("composer-toolbar").unwrap();
                    let mut names = vec![
                        "composer-add",
                        "composer-workspace",
                        "composer-branch",
                        "composer-model",
                        "composer-access",
                    ];
                    if !started {
                        names.push("context-meter");
                    }
                    let mut bounds: Vec<_> = names
                        .iter()
                        .map(|name| visual.debug_bounds(name).unwrap())
                        .collect();
                    if beside {
                        names.push("model-effort");
                        bounds.push(visual.debug_bounds("model-effort").unwrap());
                    }
                    for (i, a) in bounds.iter().enumerate() {
                        assert!(
                            a.left() >= toolbar.left()
                                && a.right() <= toolbar.right()
                                && a.top() >= toolbar.top()
                                && a.bottom() <= toolbar.bottom(),
                            "{} outside toolbar at {width}, started={started}, beside={beside}: {a:?}, {toolbar:?}",
                            names[i]
                        );
                        for (j, b) in bounds.iter().enumerate().skip(i + 1) {
                            assert!(
                                a.right() <= b.left()
                                    || b.right() <= a.left()
                                    || a.bottom() <= b.top()
                                    || b.bottom() <= a.top(),
                                "{} overlaps {} at {width}, started={started}, beside={beside}: {a:?}, {b:?}",
                                names[i],
                                names[j]
                            );
                        }
                    }
                    if !beside {
                        let effort = visual.debug_bounds("composer-model-effort").unwrap();
                        let label = visual.debug_bounds("composer-model-label").unwrap();
                        let model = bounds[3];
                        assert!(effort.left() >= model.left() && effort.right() <= model.right());
                        assert!(label.right() <= effort.left());
                    }
                    visual.simulate_click(bounds[3].center(), Default::default());
                    assert!(
                        window
                            .update(visual, |view, _, _| view.menu_is(ComposerMenu::Model)
                                || view.bot_ui.models.is_some())
                            .unwrap()
                    );
                    window
                        .update(visual, |view, _, cx| {
                            view.menu = None;
                            view.bot_ui.models = None;
                            cx.notify();
                        })
                        .unwrap();
                    visual.refresh().unwrap();
                    let access = visual.debug_bounds("composer-access").unwrap();
                    visual.simulate_click(access.center(), Default::default());
                    assert!(
                        window
                            .update(visual, |view, _, _| view.menu_is(ComposerMenu::Access))
                            .unwrap()
                    );
                    window
                        .update(visual, |view, _, cx| {
                            view.menu = None;
                            cx.notify();
                        })
                        .unwrap();
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "Requires Metal and VORKSPACE_COMPOSER_CAPTURE_DIR"]
    fn native_narrow_composer_capture() {
        struct Capture(Entity<Conversation>);
        impl gpui::Render for Capture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().bg(pane_fill()).child(self.0.clone())
            }
        }
        let output =
            std::path::PathBuf::from(std::env::var_os("VORKSPACE_COMPOSER_CAPTURE_DIR").unwrap());
        assert!(output.is_absolute() && output.is_dir());
        let mut cx = gpui::HeadlessAppContext::with_platform(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(crate::assets::Assets),
            || {
                Ok(Some(Box::new(
                    gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
                )))
            },
        );
        crate::clock::use_test_clock(&cx);
        for light in [false, true] {
            let prefs = crate::settings::Preferences {
                theme: if light { "light" } else { "dark" }.into(),
                ..Default::default()
            };
            crate::theme::configure(&prefs, false, true);
            cx.update(|cx| cx.set_global(prefs));
            for width in [260., 360., 480., 760.] {
                let store = Store::open(Path::new(":memory:")).unwrap();
                let summary = store.create("codex", "/work/vorkspace").unwrap();
                let store = Arc::new(Mutex::new(store));
                let window = cx
                    .open_window(size(px(width), px(700.)), |_, cx| {
                        let conversation = cx.new(|cx| {
                            let mut view = Conversation::new(summary, store, cx);
                            view.loaded = true;
                            view.status = "Ready".into();
                            view.branch = Some("feature/a-very-long-branch-name".into());
                            view.settings.access = AccessMode::FullAccess;
                            view.model = Some("gpt-6.1-sol".into());
                            view.settings.effort = Some("xhigh".into());
                            view.catalog = vec![Model {
                                alias_target: None,
                                id: "gpt-6.1-sol".into(),
                                name: "GPT-6.1 Sol".into(),
                                efforts: vec!["high".into(), "xhigh".into()],
                                default_effort: Some("high".into()),
                                fast: false,
                                thinking: false,
                                default: true,
                                tiers: Vec::new(),
                                controls: Vec::new(),
                            }];
                            view
                        });
                        cx.new(|_| Capture(conversation))
                    })
                    .unwrap();
                cx.run_until_parked();
                crate::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!(
                        "composer-{}-{width}.png",
                        if light { "light" } else { "dark" }
                    )))
                    .unwrap();
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
        crate::theme::configure(&Default::default(), false, true);
    }
    #[gpui::test]
    fn composer_uses_react_surface_and_equal_text_padding_with_attachments(
        cx: &mut TestAppContext,
    ) {
        let preferences = crate::settings::Preferences::default();
        crate::theme::configure(&preferences, false, true);
        cx.update(|cx| {
            cx.set_global(preferences);
            crate::clock::use_test_clock(cx);
        });
        for width in [480., 1000.] {
            let window = window(cx, width);
            let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
            for docked in [false, true] {
                for attached in [false, true] {
                    window
                        .update(visual, |c, _, cx| {
                            c.summary.has_messages = docked;
                            c.attachments = if attached {
                                vec![std::path::PathBuf::from("/work/notes.txt")]
                            } else {
                                Vec::new()
                            };
                            c.input.update(cx, |input, cx| {
                                input.set_text(
                                    "First typed line\nSecond typed line\nThird typed line".into(),
                                    cx,
                                );
                            });
                            cx.notify();
                        })
                        .unwrap();
                    visual.run_until_parked();
                    visual.refresh().unwrap();
                    let composer = visual.debug_bounds("composer-box").unwrap();
                    let field = visual.debug_bounds("composer-text-field").unwrap();
                    let toolbar = visual.debug_bounds("composer-toolbar").unwrap();
                    // The column is centred in the pane, not pushed to an edge.
                    let left = composer.left();
                    let right = px(width) - composer.right();
                    assert!(
                        (left - right).abs() <= px(1.),
                        "composer is centred: {left:?} vs {right:?} at {width}px"
                    );
                    let top = if attached {
                        visual
                            .debug_bounds("composer-attachments")
                            .unwrap()
                            .bottom()
                    } else {
                        composer.top() + px(1.)
                    };
                    // BotComposer's box: 5px around the row plus the field's
                    // 4px, and the tools sit under the box, not inside it.
                    assert_eq!(field.top() - top, px(9.));
                    assert_eq!(composer.bottom() - field.bottom(), px(10.));
                    assert_eq!(toolbar.top(), composer.bottom());
                    assert_eq!(field.size.height, px(54.));
                    window
                        .update(visual, |_, window, _| {
                            let scale = window.scale_factor();
                            let surface = window
                                .painted_quads()
                                .into_iter()
                                .find(|quad| {
                                    (quad.corner_radii.top_left.0 / scale - 7.).abs() < 0.1
                                        && quad.background.as_solid() == Some(content(0.03))
                                        && (quad.bounds.size.width.0 / scale
                                            - f32::from(composer.size.width))
                                        .abs()
                                            <= 2.
                                })
                                .expect("the composer paints React's content-3% surface");
                            assert_eq!(
                                surface.border_color,
                                gpui_apple::metal_renderer::backdrop_blur_marker(24. * scale)
                            );
                            assert!(window.painted_quads().iter().any(|quad| {
                                quad.border_color == content(0.2)
                                    && (quad.bounds.size.width.0 / scale
                                        - f32::from(composer.size.width))
                                    .abs()
                                        < 0.1
                            }));
                        })
                        .unwrap();
                }
            }
        }
    }

    #[gpui::test]
    fn first_submit_moves_the_painted_composer_down_without_resizing_chat(cx: &mut TestAppContext) {
        let window = window(cx, 1000.);
        let from = begin(window, cx);
        let (initial, transcript) = window
            .update(cx, |c, _, cx| {
                (
                    c.composer_anchor.get().unwrap(),
                    c.transcript.read(cx).list.viewport_bounds(),
                )
            })
            .unwrap();
        assert!(
            (initial.top() - from.top()).abs() < px(1.),
            "the first docked frame must stay at the empty-session slot"
        );
        window
            .update(cx, |c, _, cx| {
                c.composer_motion.elapsed(Duration::from_millis(120));
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        let middle = window
            .update(cx, |c, w, cx| {
                let bounds = c.composer_anchor.get().unwrap();
                let painted = w
                    .painted_quads()
                    .into_iter()
                    .find(|q| {
                        (q.corner_radii.top_left.0 / w.scale_factor() - 8.).abs() < 0.1
                            && q.bounds.size.width.0 / w.scale_factor() > 500.
                    })
                    .expect("the composer box must be painted at its animated position");
                assert!(
                    (painted.bounds.top().0 / w.scale_factor() - f32::from(bounds.top())).abs()
                        < 1.
                );
                assert!(c.input.focus_handle(cx).is_focused(w));
                c.composer_motion.elapsed(Duration::from_millis(480));
                cx.notify();
                bounds
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        let docked = window
            .update(cx, |c, _, _| c.composer_anchor.get().unwrap())
            .unwrap();
        assert!(
            middle.top() > from.top() + px(30.),
            "the composer must slide down"
        );
        assert!(
            middle.top() < docked.top() - px(5.),
            "an intermediate frame must not already be docked"
        );
        assert!((docked.bottom() - px(694.)).abs() < px(1.));
        assert_eq!(
            window
                .update(cx, |c, _, cx| c.transcript.read(cx).list.viewport_bounds())
                .unwrap(),
            transcript,
            "composer motion must not relayout the transcript"
        );
    }
    #[gpui::test]
    fn narrow_layout_and_opening_history_do_not_slide(cx: &mut TestAppContext) {
        let narrow = window(cx, 820.);
        narrow
            .update(cx, |c, _, cx| c.set_beside_pane(true, cx))
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        let from = begin(narrow, cx);
        narrow
            .update(cx, |c, _, _| {
                assert!((c.composer_anchor.get().unwrap().top() - from.top()).abs() < px(1.));
            })
            .unwrap();
        let history = window(cx, 1000.);
        let from = history
            .update(cx, |c, _, cx| {
                let from = c.composer_anchor.get().unwrap();
                c.summary.has_messages = true;
                cx.notify();
                from
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        history
            .update(cx, |c, _, _| {
                assert!(c.composer_anchor.get().unwrap().top() > from.top() + px(100.));
            })
            .unwrap();
    }
}
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
mod menu_tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle, point};
    use std::sync::Mutex;
    use vorkspace_core::store::Store;
    #[gpui::test]
    fn combined_picker_paints_effort_and_beside_mode_moves_it_to_its_own_control(
        cx: &mut TestAppContext,
    ) {
        let window = conversation(cx);
        window
            .update(cx, |view, _, cx| {
                view.summary.provider = "codex".into();
                view.model = Some("gpt-6.1-sol".into());
                view.settings.effort = Some("xhigh".into());
                view.catalog = vec![Model {
                    alias_target: None,
                    id: "gpt-6.1-sol".into(),
                    name: "GPT-6.1 Sol".into(),
                    efforts: vec!["high".into(), "xhigh".into()],
                    default_effort: Some("high".into()),
                    fast: false,
                    thinking: false,
                    default: true,
                    tiers: Vec::new(),
                    controls: Vec::new(),
                }];
                cx.set_global(crate::settings::Preferences::default());
                cx.notify();
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        let effort = visual
            .debug_bounds("composer-model-effort")
            .expect("combined model picker includes effort");
        let model = window
            .update(visual, |view, _, _| view.model_anchor.get().unwrap())
            .unwrap();
        assert!(effort.left() > model.left() && effort.right() < model.right());
        window
            .update(visual, |_, _, cx| {
                cx.global_mut::<crate::settings::Preferences>()
                    .model_controls = "beside".into();
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("composer-model-effort").is_none());
        window
            .update(visual, |view, _, _| {
                assert!(
                    view.bot_ui.setting_anchors[0].get().is_some(),
                    "beside mode paints its effort control"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn composer_labels_use_the_saved_model_effort_and_the_catalog_default(cx: &mut TestAppContext) {
        let window = conversation(cx);
        window
            .update(cx, |view, _, _| {
                view.summary.provider = "codex".into();
                view.model = Some("gpt-6.1-sol".into());
                view.settings.effort = Some("xhigh".into());
                view.catalog.clear();
                assert_eq!(view.model_label(), "GPT-6.1-Sol");
                assert_eq!(view.trigger_effort_label().as_deref(), Some("Extra High"));
                view.catalog.push(Model {
                    alias_target: None,
                    id: "gpt-6.1-sol".into(),
                    name: "GPT-6.1 Sol".into(),
                    efforts: vec!["high".into(), "xhigh".into()],
                    default_effort: Some("high".into()),
                    fast: false,
                    thinking: false,
                    default: true,
                    tiers: Vec::new(),
                    controls: Vec::new(),
                });
                assert_eq!(view.model_label(), "GPT-6.1 Sol");
                assert_eq!(view.trigger_effort_label().as_deref(), Some("Extra High"));
                view.model = None;
                view.settings.effort = None;
                assert_eq!(view.model_label(), "GPT-6.1 Sol");
                assert_eq!(view.trigger_effort_label().as_deref(), Some("High"));
                view.model = Some("gpt-custom-model".into());
                assert_eq!(view.model_label(), "GPT-custom-model");
                assert!(
                    view.model_info().is_none(),
                    "unknown saved models must not inherit a different model's controls"
                );
                assert!(view.trigger_effort_label().is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn tool_paths_follow_restored_and_transferred_working_copies(cx: &mut TestAppContext) {
        let window = conversation(cx);
        window
            .update(cx, |view, _, cx| {
                view.loaded = false;
                view.settings.revision = 0;
                view.restore_worktree(Some("/work/feature".into()), None, cx);
                assert_eq!(
                    view.transcript.read(cx).file_link_base.as_deref(),
                    Some("/work/feature")
                );
                // History/transfer/workspace refreshes all pass through sync_review.
                view.settings.worktree_cwd = Some("/work/transferred".into());
                view.sync_review(cx);
                assert_eq!(
                    view.transcript.read(cx).file_link_base.as_deref(),
                    Some("/work/transferred")
                );
                view.settings.worktree_removed = true;
                view.sync_review(cx);
                assert_eq!(
                    view.transcript.read(cx).file_link_base.as_deref(),
                    Some("/work/vorkspace")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn response_target_picker_preserves_model_effort_and_working_copy(cx: &mut TestAppContext) {
        let window = conversation(cx);
        let picked = std::rc::Rc::new(std::cell::RefCell::new(None));
        let result = picked.clone();
        let _subscription = window
            .update(cx, |view, window, cx| {
                let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Transfer, _| {
                    *result.borrow_mut() = Some(event.clone())
                });
                view.summary.has_messages = true;
                view.settings.worktree_cwd = Some("/work/isolated-copy".into());
                view.transcript.update(cx, |transcript, cx| {
                    transcript.patch(
                        0,
                        vec![
                            vorkspace_core::transcript::Row {
                                role: "user".into(),
                                text: "Review the footer".into(),
                                first: true,
                                sent_at: None,
                            },
                            vorkspace_core::transcript::Row {
                                role: "assistant".into(),
                                text: "The footer is ready.".into(),
                                first: true,
                                sent_at: None,
                            },
                        ],
                        cx,
                    )
                });
                let model = Model {
                    alias_target: None,
                    id: "gpt-review".into(),
                    name: "Review model".into(),
                    efforts: vec!["low".into(), "high".into()],
                    default_effort: Some("low".into()),
                    fast: false,
                    thinking: false,
                    default: true,
                    tiers: Vec::new(),
                    controls: vec![catalog::ModelControl {
                        id: "context".into(),
                        name: "Context".into(),
                        current: "1m".into(),
                        choices: vec![("1m".into(), "1M".into())],
                    }],
                };
                view.make_response_menu(
                    1,
                    vorkspace_core::response::TransferKind::Handoff,
                    AnchorSlot::default(),
                    Some(vec![(Kind::Codex, Some(Ok(vec![model])))]),
                    window,
                    cx,
                );
                cx.notify();
                subscription
            })
            .unwrap();
        cx.run_until_parked();
        cx.simulate_keystrokes(window.into(), "right right down enter");
        cx.run_until_parked();
        let result = picked.borrow();
        let result = result
            .as_ref()
            .expect("a target is picked through provider, model and effort");
        assert_eq!(result.target.provider, "codex");
        assert_eq!(result.target.model.as_deref(), Some("gpt-review"));
        assert_eq!(result.target.effort.as_deref(), Some("low"));
        assert_eq!(
            result.target.settings.get("context").map(String::as_str),
            Some("1m")
        );
        assert_eq!(
            result.settings.worktree_cwd.as_deref(),
            Some("/work/isolated-copy")
        );
        assert_eq!(
            vorkspace_core::response::request(&result.turn),
            "Review the footer"
        );
    }

    #[gpui::test]
    fn new_tab_shows_its_folders_branch_at_once_and_no_loading_status(cx: &mut TestAppContext) {
        crate::git_cache::remember(
            "/work/remembered",
            &vorkspace_core::git::Snapshot {
                current: Some("main".into()),
                ..Default::default()
            },
        );
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("codex", "/work/remembered").unwrap();
        let store = Arc::new(Mutex::new(store));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut c = Conversation::new(summary, store, cx);
                    // The first frame of a tab whose history is still loading.
                    c.loading = true;
                    c
                })
            })
            .unwrap()
        });
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |c, _, _| {
                assert_eq!(c.branch.as_deref(), Some("main"));
                assert!(c.git.is_some());
            })
            .unwrap();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("provider-status").is_none());
        // Once loaded, a real status (an error here) still shows.
        window
            .update(visual, |c, _, cx| {
                c.loading = false;
                c.status = "Couldn't load".into();
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("provider-status").is_some());
    }

    #[gpui::test]
    fn picked_access_becomes_the_default_for_new_chats(cx: &mut TestAppContext) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let first = store.create("codex", "/work").unwrap();
        let second = store.create("codex", "/work").unwrap();
        let store = Arc::new(Mutex::new(store));
        cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Conversation::new(first, store.clone(), cx))
            })
            .unwrap()
        });
        window
            .update(cx, |c, _, cx| {
                assert_eq!(c.settings.access, AccessMode::Supervised);
                c.menu_choice("access:auto", cx);
            })
            .unwrap();
        cx.update(|cx| {
            assert_eq!(
                cx.global::<crate::settings::Preferences>()
                    .default_access
                    .as_deref(),
                Some("auto")
            );
            let next = cx.new(|cx| Conversation::new(second, store, cx));
            assert_eq!(next.read(cx).settings.access, AccessMode::Auto);
        });
    }

    #[gpui::test]
    fn effort_choices_survive_provider_switches_and_new_projects(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
        let window = conversation(cx);
        window
            .update(cx, |c, _, cx| {
                c.menu_choice("effort:medium", cx);
                c.apply_model_pick(&ModelPicked(Kind::Codex, Some("gpt-6.1-sol".into())), cx);
                c.menu_choice("effort:xhigh", cx);
                c.apply_model_pick(
                    &ModelPicked(Kind::Claude, Some("claude-opus-5-5".into())),
                    cx,
                );
                assert_eq!(c.settings.effort.as_deref(), Some("medium"));
                c.apply_model_pick(&ModelPicked(Kind::Codex, Some("gpt-6.1-sol".into())), cx);
                assert_eq!(c.settings.effort.as_deref(), Some("xhigh"));
                c.menu_choice("access:auto", cx);
                let encoded =
                    serde_json::to_string(cx.global::<crate::settings::Preferences>()).unwrap();
                let restored: crate::settings::Preferences =
                    serde_json::from_str(&encoded).unwrap();
                assert_eq!(restored.provider_efforts["claude"], "medium");
                assert_eq!(restored.provider_efforts["codex"], "xhigh");
                assert_eq!(restored.default_access.as_deref(), Some("auto"));
                let store = Store::open(Path::new(":memory:")).unwrap();
                let summary = store.create("codex", "/different/project").unwrap();
                let next = cx.new(|cx| Conversation::new(summary, Arc::new(Mutex::new(store)), cx));
                assert_eq!(next.read(cx).settings.effort.as_deref(), Some("xhigh"));
                assert_eq!(next.read(cx).settings.access, AccessMode::Auto);
            })
            .unwrap();
    }

    fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("claude", "/work/vorkspace").unwrap();
        let store = Arc::new(Mutex::new(store));
        let window = cx.update(|cx| {
            input::bind(cx);
            crate::popover::bind(cx);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Conversation::new(summary, store, cx))
            })
            .unwrap()
        });
        window
            .update(cx, |c, w, cx| w.focus(&c.input.focus_handle(cx), cx))
            .unwrap();
        cx.run_until_parked();
        window
    }
    fn open(window: WindowHandle<Conversation>, kind: ComposerMenu, cx: &mut TestAppContext) {
        window
            .update(cx, |c, w, cx| c.toggle_menu(kind, w, cx))
            .unwrap();
        cx.run_until_parked();
    }
    fn input_focused(window: WindowHandle<Conversation>, cx: &mut TestAppContext) -> bool {
        window
            .update(cx, |c, w, cx| c.input.focus_handle(cx).is_focused(w))
            .unwrap()
    }

    #[gpui::test]
    fn workspace_existing_worktrees_open_as_a_submenu_and_preserve_the_draft(
        cx: &mut TestAppContext,
    ) {
        use vorkspace_core::git::{Snapshot, Worktree};
        cx.update(|cx| {
            input::bind(cx);
            crate::popover::bind(cx);
            crate::clock::use_test_clock(cx);
        });
        let cwd = "/nonexistent/vorkspace-composer-existing-submenu";
        let feature = format!("{cwd}-feature-b");
        crate::git_cache::remember(
            cwd,
            &Snapshot {
                current: Some("main".into()),
                worktrees: vec![
                    Worktree {
                        path: cwd.into(),
                        branch: Some("main".into()),
                        main: true,
                        ..Default::default()
                    },
                    Worktree {
                        path: format!("{cwd}-feature-a"),
                        branch: Some("feature-a".into()),
                        ..Default::default()
                    },
                    Worktree {
                        path: feature.clone(),
                        branch: Some("feature-b".into()),
                        ..Default::default()
                    },
                    Worktree {
                        path: format!("{cwd}-missing"),
                        branch: Some("missing".into()),
                        missing: true,
                        ..Default::default()
                    },
                    Worktree {
                        path: format!("{cwd}-detached"),
                        head: "123456789abc".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        );
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("claude", cwd).unwrap();
        let store = Arc::new(Mutex::new(store));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Conversation::new(summary, store, cx))
            })
            .unwrap()
        });
        window
            .update(cx, |c, w, cx| {
                c.input.update(cx, |input, cx| {
                    input.set_text("Keep my unsent draft".into(), cx)
                });
                c.settings.new_worktree = true;
                c.toggle_menu(ComposerMenu::Workspace, w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes("down down right");
        visual.run_until_parked();
        let parent = visual
            .debug_bounds("menu-action-workspace:existing")
            .unwrap();
        let submenu = visual
            .debug_bounds("existing-worktrees-submenu")
            .expect("existing copies open beside the workspace menu");
        assert!(submenu.left() >= parent.right());
        assert!(submenu.size.height > px(0.) && submenu.size.height <= px(320.));
        assert!(
            visual.debug_bounds("git-option-2").is_some(),
            "detached copies remain available"
        );
        assert!(
            visual.debug_bounds("git-option-3").is_none(),
            "main and missing copies are excluded"
        );
        window
            .update(visual, |c, _, _| {
                assert!(c.menu_is(ComposerMenu::Workspace));
                assert!(
                    c.git_picker.is_none(),
                    "the submenu never replaces its parent"
                );
            })
            .unwrap();

        visual.simulate_keystrokes("left");
        visual.run_until_parked();
        assert!(visual.debug_bounds("existing-worktrees-submenu").is_none());
        assert!(
            visual
                .debug_bounds("menu-action-workspace:existing")
                .is_some()
        );
        visual.simulate_keystrokes("right escape");
        visual.run_until_parked();
        window
            .update(visual, |c, w, cx| {
                assert!(c.menu.is_none());
                assert!(c.input.focus_handle(cx).is_focused(w));
                assert!(
                    c.settings.new_worktree,
                    "dismissal does not change workspace mode"
                );
                c.toggle_menu(ComposerMenu::Workspace, w, cx);
            })
            .unwrap();
        visual.run_until_parked();
        let existing = visual
            .debug_bounds("menu-action-workspace:existing")
            .unwrap();
        visual.simulate_mouse_move(existing.center(), None, Default::default());
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("existing-worktrees-submenu").is_some(),
            "hover also opens the submenu"
        );
        visual.simulate_input("feature-b");
        visual.run_until_parked();
        assert!(visual.debug_bounds("git-option-0").is_some());
        assert!(visual.debug_bounds("git-option-1").is_none());
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        window
            .update(visual, |c, w, cx| {
                assert!(c.menu.is_none());
                assert_eq!(c.execution_cwd(), feature);
                assert_eq!(
                    c.transcript.read(cx).file_link_base.as_deref(),
                    Some(feature.as_str())
                );
                assert!(!c.settings.new_worktree);
                assert!(!c.busy);
                assert_eq!(c.input.read(cx).content.as_ref(), "Keep my unsent draft");
                assert!(c.input.focus_handle(cx).is_focused(w));
                assert_eq!(
                    locked(&c.store)
                        .unwrap()
                        .agent_settings(c.summary.id)
                        .unwrap()
                        .unwrap()
                        .worktree_cwd
                        .as_deref(),
                    Some(feature.as_str())
                );
                c.toggle_menu(ComposerMenu::Workspace, w, cx);
            })
            .unwrap();
        visual.run_until_parked();
        let current = visual
            .debug_bounds("menu-action-workspace:current")
            .unwrap();
        visual.simulate_click(current.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |c, _, cx| {
                assert!(c.settings.worktree_cwd.is_none());
                assert_eq!(c.execution_cwd(), cwd);
                assert_eq!(c.input.read(cx).content.as_ref(), "Keep my unsent draft");
            })
            .unwrap();
    }

    #[gpui::test]
    fn mode_menu_edits_the_inline_prefix_and_removal_preserves_the_draft(cx: &mut TestAppContext) {
        use gpui::EntityInputHandler;
        let window = conversation(cx);
        window
            .update(cx, |c, _, cx| {
                c.input
                    .update(cx, |i, cx| i.set_text("Review this".into(), cx));
                c.toggle_mode("plan", cx);
                assert_eq!(c.input.read(cx).content, "/plan Review this");
                c.toggle_mode("operator", cx);
                assert_eq!(c.input.read(cx).content, "/operator Review this");
                c.toggle_mode("orchestrator", cx);
                assert_eq!(c.input.read(cx).content, "/orchestrator Review this");
                c.toggle_mode("orchestrator", cx);
                assert_eq!(c.input.read(cx).content, "Review this");
                c.toggle_mode("plan", cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |c, w, cx| {
                c.input.update(cx, |i, cx| {
                    i.select_range(0..i.content.len(), cx);
                    i.replace_text_in_range(None, "/operator New draft", w, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |c, _, cx| {
                assert!(!c.settings.plan);
                assert_eq!(c.input.read(cx).content, "/operator New draft");
                c.enable_mode("operator", cx);
                c.input.update(cx, |i, cx| i.select_range(1..1, cx));
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "backspace");
        cx.run_until_parked();
        window
            .update(cx, |c, _, cx| {
                assert!(!c.settings.operator);
                assert_eq!(c.input.read(cx).content, "operator New draft");
            })
            .unwrap();
    }

    #[gpui::test]
    fn submitted_modes_clear_for_a_started_turn_and_stay_cleared_after_completion(
        cx: &mut TestAppContext,
    ) {
        for mode in ["operator", "orchestrator", "plan"] {
            let window = conversation(cx);
            window
                .update(cx, |view, _, cx| {
                    // An absent directory prevents the supported CLI from starting.
                    view.summary.provider = "codex".into();
                    view.summary.cwd = "/missing/submitted-mode-project".into();
                    view.loaded = true;
                    view.input
                        .update(cx, |i, cx| i.set_text("check the composer".into(), cx));
                    view.toggle_mode(mode, cx);
                    view.send(cx);
                    assert!(view.busy);
                    assert!(view.input.read(cx).content.is_empty());
                    assert!(
                        !view.settings.plan
                            && !view.settings.operator
                            && !view.settings.orchestrator
                    );
                })
                .unwrap();
            cx.run_until_parked();
            window
                .update(cx, |view, _, cx| {
                    let saved = locked(&view.store)
                        .unwrap()
                        .agent_settings(view.summary.id)
                        .unwrap()
                        .unwrap();
                    assert!(!saved.plan && !saved.operator && !saved.orchestrator);
                    assert!(view.input.read(cx).content.is_empty());
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn model_menu_picks_by_keyboard_and_returns_focus(cx: &mut TestAppContext) {
        let window = conversation(cx);
        open(window, ComposerMenu::Model, cx);
        assert!(
            !input_focused(window, cx),
            "the menu takes focus while open"
        );
        // Open the Model row's flyout, which focuses its search field with
        // Default highlighted, then take the first catalog model.
        cx.simulate_keystrokes(window.into(), "right down enter");
        cx.run_until_parked();
        window
            .update(cx, |c, _, _| {
                assert!(c.menu.is_none());
                assert_eq!(c.model.as_deref(), Some("sonnet"));
                // The pill drops the catalog's "Claude " prefix, as React's does.
                assert_eq!(c.model_label(), "Sonnet");
            })
            .unwrap();
        assert!(input_focused(window, cx));

        open(window, ComposerMenu::Model, cx);
        cx.simulate_keystrokes(window.into(), "escape");
        cx.run_until_parked();
        window
            .update(cx, |c, _, _| {
                assert!(c.menu.is_none());
                assert_eq!(c.model.as_deref(), Some("sonnet"));
            })
            .unwrap();
        assert!(input_focused(window, cx));
    }

    #[gpui::test]
    fn recent_models_open_on_right_click_and_pick_by_keyboard(cx: &mut TestAppContext) {
        let window = conversation(cx);
        window
            .update(cx, |c, _, cx| {
                c.model = Some("opus".into());
                let prefs = crate::settings::Preferences {
                    recent_models: [
                        ("codex", Some("gpt-6.1-sol")),
                        ("claude", Some("opus")),
                        ("claude", None),
                        ("codex", Some("gpt-6-luna")),
                    ]
                    .into_iter()
                    .map(|(provider, model)| crate::settings::ModelChoice {
                        provider: provider.into(),
                        model: model.map(Into::into),
                    })
                    .collect(),
                    hidden_models: vec![
                        crate::settings::model_key("codex", "gpt-6.1-sol"),
                        crate::settings::model_key("claude", "opus"),
                    ],
                    ..Default::default()
                };
                cx.set_global(prefs);
                cx.notify();
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let pill = window
            .update(visual, |c, _, _| c.model_anchor.get().unwrap())
            .unwrap()
            .center();
        visual.simulate_event(gpui::MouseDownEvent {
            position: pill,
            button: MouseButton::Right,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        });
        visual.run_until_parked();
        window
            .update(visual, |c, _, cx| {
                assert!(c.menu_is(ComposerMenu::Recent));
                // Recents with a model, in order; the default is not one.
                let items = c.recent_model_items(cx);
                let labels: Vec<_> = items
                    .iter()
                    .filter_map(|item| match item {
                        MenuItem::Action(a) => Some((a.label.to_string(), a.checked)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    labels,
                    [
                        ("Opus".to_string(), true),
                        ("GPT-6-luna".to_string(), false),
                    ]
                );
            })
            .unwrap();
        // The current model is highlighted; Down then Enter takes the next.
        visual.simulate_keystrokes("down enter");
        visual.run_until_parked();
        window
            .update(visual, |c, _, cx| {
                assert!(c.menu.is_none());
                assert_eq!(c.summary.provider, "codex");
                assert_eq!(c.model.as_deref(), Some("gpt-6-luna"));
                let recent = &cx.global::<crate::settings::Preferences>().recent_models[0];
                assert_eq!(recent.model.as_deref(), Some("gpt-6-luna"));
                // App: Switch Model toggles the same menu.
            })
            .unwrap();
        window
            .update(visual, |c, w, cx| c.toggle_recent_models(w, cx))
            .unwrap();
        window
            .update(visual, |c, _, _| assert!(c.menu_is(ComposerMenu::Recent)))
            .unwrap();
        window
            .update(visual, |c, w, cx| c.toggle_recent_models(w, cx))
            .unwrap();
        window
            .update(visual, |c, _, _| assert!(c.menu.is_none()))
            .unwrap();
    }

    #[gpui::test]
    fn pressing_outside_dismisses_and_the_trigger_toggles(cx: &mut TestAppContext) {
        let window = conversation(cx);
        open(window, ComposerMenu::Access, cx);
        cx.update_window(window.into(), |_, w, cx| {
            w.dispatch_event(
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    position: point(px(1.), px(1.)),
                    button: MouseButton::Left,
                    modifiers: Default::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |c, _, _| assert!(c.menu.is_none()))
            .unwrap();
        open(window, ComposerMenu::Add, cx);
        open(window, ComposerMenu::Add, cx);
        window
            .update(cx, |c, _, _| assert!(c.menu.is_none()))
            .unwrap();
        assert!(input_focused(window, cx));
    }
}
