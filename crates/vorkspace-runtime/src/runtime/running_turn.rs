use super::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use vorkspace_core::{
    accounts, bot, checkpoint, native_history as provider_history, orchestration, process,
    ticket_context, worktree_management, worktree_setup,
};
use vorkspace_core::{
    agent::{Approval, BotContext, ConfigurationPatch, Handoff, Question, TurnInfo, TurnMetrics},
    context::Usage,
    follow_up::Message,
    store::ProviderSource,
    tool::{ToolCall, ToolState},
    usage::Limit,
    visual_bridge::{Publication, Server},
    worktree_setup::Notice,
};
mod events;
struct RunningTurn {
    input: TurnInput,
    session: i64,
    account: String,
    user_prompt: String,
    transcript: Transcript,
    history_loaded: bool,
    recovery_id: String,
    pending: PendingSave,
    submissions: submissions::Submissions,
    provider_completed: bool,
    delivered: Option<String>,
    accepted_ids: Vec<String>,
    provider_id: Option<String>,
    binding: ProviderBinding,
    provider_reset: bool,
    start: usize,
    activity: activity::ToolActivity,
    turn_info: Option<(usize, TurnInfo)>,
    bot_context: Option<BotContextReceipt>,
    bot: Option<BotContext>,
    discovery: Option<visual_discovery::Discovery>,
    handoff: Option<Handoff>,
    configuration: Option<ProviderConfiguration>,
    revision: u64,
    handoff_accepted: bool,
    handoff_retired: bool,
    // SessionReview's recorded turn: its project and id, once the baseline is taken.
    checkpoint: Option<(String, String)>,
    created_worktree: Option<String>,
    context: Usage,
    usage_limit: Option<Limit>,
    store: Database,
    cancel: Arc<AtomicBool>,
    tx: async_channel::Sender<Update>,
    tool_rows: HashMap<String, usize>,
    update: Update,
    from: usize,
    context_changed: bool,
    completed_batch: bool,
    boundary: bool,
}
impl RunningTurn {
    fn publish(&self, update: Update) -> Result<(), String> {
        publish(&self.tx, &self.cancel, update)
    }
    fn source(&self) -> ProviderSource<'_> {
        ProviderSource {
            session: self.session,
            provider: &self.input.summary.provider,
            account: &self.account,
            provider_id: self.provider_id.as_deref(),
            revision: self.revision,
        }
    }
    fn flush(&mut self, now: Instant) -> Result<(), String> {
        self.pending.flush(
            &self.store,
            self.session,
            &self.transcript,
            &self.context,
            now,
        )
    }
    fn new(
        mut input: TurnInput,
        store: Database,
        cancel: Arc<AtomicBool>,
        tx: async_channel::Sender<Update>,
    ) -> Self {
        let settings = &mut input.settings;
        if settings.account.root.is_none() && settings.account.id != accounts::DEFAULT {
            settings.account.root = locked(&store)
                .ok()
                .and_then(|s| s.data_dir().map(Path::to_path_buf));
        }
        let mut turn = Self {
            account: settings.account.id.clone(),
            user_prompt: input.prompt.clone(),
            transcript: Transcript::default(),
            history_loaded: false,
            recovery_id: bot::new_id(),
            pending: PendingSave::new(Instant::now()),
            submissions: submissions::Submissions::new(store.clone(), input.summary.id),
            provider_completed: false,
            delivered: None,
            accepted_ids: Vec::new(),
            provider_id: input.summary.provider_id.clone(),
            binding: ProviderBinding::Unchanged,
            provider_reset: false,
            start: 0,
            activity: activity::ToolActivity::default(),
            turn_info: None,
            bot_context: None,
            bot: settings.bot.clone(),
            discovery: None,
            handoff: None,
            configuration: None,
            revision: settings.revision,
            handoff_accepted: false,
            handoff_retired: false,
            checkpoint: None,
            created_worktree: None,
            context: Usage::default(),
            usage_limit: None,
            store,
            cancel,
            tx,
            session: input.summary.id,
            input,
            tool_rows: Default::default(),
            update: Default::default(),
            from: 0,
            context_changed: false,
            completed_batch: false,
            boundary: false,
        };
        turn.pending.own_context(
            &turn.input.summary.provider,
            &turn.account,
            turn.input.summary.provider_id.as_deref(),
        );
        turn.context = locked(&turn.store)
            .ok()
            .and_then(|s| s.context_usage(turn.session).ok().flatten())
            .unwrap_or_default();
        turn
    }
    fn prepare_history(&mut self) -> Result<Kind, String> {
        let settings = &mut self.input.settings;
        let summary = &self.input.summary;
        if self.input.composer_origin.is_some() && self.cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        let kind = Kind::parse(&summary.provider).ok_or_else(|| {
            format!(
                "Provider {} is no longer supported; its saved history is still available",
                summary.provider
            )
        })?;
        if let Some(reason) = locked(&self.store)?.edit_recovery_reason(summary.id)? {
            return Err(reason);
        }
        if settings.worktree_removed {
            return Err("Select a branch or worktree to continue this conversation.".into());
        }
        if settings.account.id != accounts::DEFAULT
            && !locked(&self.store)?
                .provider_accounts()?
                .exists(Kind::from_id(&summary.provider), &settings.account.id)
        {
            return Err(
                "This provider account was removed. Choose another account to continue.".into(),
            );
        }
        let native_history =
            native_history::before_turn(&self.store, summary, settings, &self.cancel)?;
        if settings.bot.is_none() {
            let store = locked(&self.store)?;
            self.context = store.context_usage(summary.id)?.unwrap_or_default();
            (self.transcript, self.pending.writer) = store.load_turn_history(summary.id)?;
            self.history_loaded = true;
            self.start = self.transcript.rows.len();
        }
        settings.ticket_context_data_dir = locked(&self.store)?.data_dir().map(Path::to_path_buf);
        settings.ticket_context = Some(ticket_context::admit(
            &self.input.prompt,
            settings.ticket_context_data_dir.as_deref(),
            Path::new(settings.worktree_cwd.as_deref().unwrap_or(&summary.cwd)),
            &self.cancel,
            settings.ticket_context.take(),
        )?);
        if let Err(error) = self.submissions.reserve_initial(
            &self.input.prompt,
            settings,
            native_history.as_ref(),
            self.input.composer_origin.as_ref(),
        ) {
            // Rejected admission owns no transcript rows, even when the
            // ordinary chat loaded a read baseline during preparation.
            self.history_loaded = false;
            return Err(error);
        }
        if settings.bot.is_some() {
            // Reservation excludes background import before this writer takes
            // its baseline, including imports finishing during preparation.
            {
                let store = locked(&self.store)?;
                let current = store
                    .bot_session(summary.id)?
                    .ok_or("The Bot session no longer exists")?;
                let saved = store.agent_settings(summary.id)?.unwrap_or_default();
                if current.provider != summary.provider
                    || current.cwd != summary.cwd
                    || saved.account.id != self.account
                {
                    return Err("The Bot conversation changed before this turn. Retry to use its current settings.".into());
                }
                self.provider_reset = self.provider_id != current.provider_id;
                self.provider_id = current.provider_id;
                if self.provider_reset {
                    self.binding = ProviderBinding::Set(self.provider_id.clone());
                }
                self.pending.own_context_thread(self.provider_id.as_deref());
                self.context = store.context_usage(summary.id)?.unwrap_or_default();
                (self.transcript, self.pending.writer) = store.load_turn_history(summary.id)?;
                settings.turns = saved.turns;
            }
            self.history_loaded = true;
            self.start = self.transcript.rows.len();
            if kind == Kind::Claude {
                settings.native_prompt_id = Some(provider_history::input_uuid(&bot::new_id()));
            }
            let raw_native_command = kind == Kind::Omp
                && provider::native_prompt(
                    kind,
                    settings
                        .prepared_prompt
                        .as_deref()
                        .unwrap_or(&self.input.prompt),
                )
                .trim_start()
                .starts_with('/');
            let rollover = !raw_native_command
                && settings
                    .bot
                    .as_ref()
                    .is_some_and(|b| b.bot.compaction_mode == bot::CompactionMode::SessionRollover)
                && self.provider_id.is_some()
                && self.context.ratio().is_some_and(|ratio| ratio >= 0.8);
            if rollover || (self.provider_id.is_none() && self.context != Usage::default()) {
                let previous = self.provider_id.clone();
                if !locked(&self.store)?.replace_bot_provider_session(
                    summary.id,
                    &summary.provider,
                    &self.account,
                    previous.as_deref(),
                    None,
                )? {
                    return Err("The Bot session changed before context rollover. Retry to continue its current session.".into());
                }
                self.provider_id = None;
                self.binding = ProviderBinding::Set(None);
                self.pending.own_context_thread(None);
                self.context = Usage::default();
                self.provider_reset = true;
                if let Some(previous) = previous {
                    provider::forget_session(
                        kind,
                        &previous,
                        &self.account,
                        Path::new(&summary.cwd),
                    );
                }
                self.publish(Update {
                    binding: self.binding.clone(),
                    context: Some(self.context),
                    status: "Starting fresh context…".into(),
                    ..Default::default()
                })?;
            }
        }
        Ok(kind)
    }
    fn save_prompt(&mut self) -> Result<bool, String> {
        let settings = &self.input.settings;
        let summary = &self.input.summary;
        let mut persisted = settings.clone();
        // These modes belong to this turn, not the next composer draft.
        persisted.plan = false;
        persisted.operator = false;
        persisted.orchestrator = false;
        if self.pending.writer.is_some() {
            persisted.turns.clear();
        }
        let start = self.transcript.begin(if settings.internal_message {
            "bot-system"
        } else {
            "user"
        });
        if let Some(at) = settings
            .turn_model
            .as_ref()
            .and_then(|turn| turn.started_at)
        {
            self.transcript.rows[start].sent_at = Some(at);
        }
        if settings.bot.is_some()
            && let Some(at) = settings.bot_sent_at
        {
            self.transcript.rows[start].sent_at = Some(at);
        }
        let started_at = self.transcript.rows[start].sent_at;
        self.transcript.append(&self.input.prompt);
        self.transcript.attach_files(&settings.attachments);
        // Show the working receipt while connecting, before the first token.
        self.transcript.begin("assistant");
        // Keep the already painted prompt in the failure replay even if saving
        // the turn's configuration fails before its first transcript write.
        locked(&self.store)?.configure_agent(summary.id, &summary.provider, &persisted)?;
        self.pending.write(
            &mut *locked(&self.store)?,
            summary.id,
            start,
            &self.transcript.rows[start..],
            settings.bot_message_id.as_deref(),
        )?;
        // A retained recap belongs to this admitted request until the provider
        // accepts it. A failed restart must leave the packet ready for retry.
        self.handoff = settings.handoff.clone();
        let mut info = settings.turn_model.clone().unwrap_or_else(|| TurnInfo {
            provider: summary.provider.clone(),
            model_id: settings.model.clone(),
            ..Default::default()
        });
        info.started_at = info.started_at.or(started_at);
        if let Some(id) = &settings.native_prompt_id {
            info.provider_turn_id = Some(id.clone());
        }
        // The receipt's key follows all continuation rows of the prompt.
        let turn = self
            .transcript
            .rows
            .iter()
            .rposition(Row::is_prompt)
            .map_or(0, |at| at + 1);
        self.pending
            .receipt(&self.store, summary.id, turn, info.clone())?;
        self.turn_info = Some((turn, info));
        self.delivered = settings.bot_message_id.clone();
        let creating_worktree = settings.new_worktree && settings.worktree_cwd.is_none();
        let bot_snapshot = self.bot.is_some();
        self.publish(Update {
            activity: self.activity.label(),
            delivered: settings.bot_message_id.clone(),
            // A native import may have committed while its idle UI refresh
            // was superseded by this submission. Publish the admitted Bot
            // baseline together with its prompt, never a tail over stale rows.
            transcript: if self.input.composer_origin.is_some() {
                TranscriptPatch::Unchanged
            } else if bot_snapshot {
                TranscriptPatch::BotSnapshot {
                    turn_start: self.start,
                    rows: self.transcript.rows.clone(),
                }
            } else {
                TranscriptPatch::tail(start, self.transcript.rows[start..].to_vec())
            },
            binding: self.binding.clone(),
            context: self.provider_reset.then_some(self.context),
            status: if creating_worktree {
                "Creating worktree…"
            } else if settings.pending_provider_switch.is_some() {
                "Preparing handoff…"
            } else {
                "Connecting…"
            }
            .into(),
            ..Default::default()
        })?;
        Ok(creating_worktree)
    }
    fn prepare_worktree(&mut self, creating_worktree: bool) -> Result<Option<Notice>, String> {
        let settings = &mut self.input.settings;
        let summary = &self.input.summary;
        if creating_worktree {
            let path = vorkspace_core::git::create_worktree(
                Path::new(&summary.cwd),
                settings.worktree_base.as_deref().unwrap_or("HEAD"),
                summary.id,
            )?;
            let cwd = path.to_string_lossy().into_owned();
            self.created_worktree = Some(cwd.clone());
            settings.worktree_cwd = Some(cwd.clone());
            settings.new_worktree = false;
            {
                let store = locked(&self.store)?;
                let mut persisted = store.agent_preferences(summary.id)?.unwrap_or_default();
                persisted.worktree_cwd = Some(cwd.clone());
                persisted.new_worktree = false;
                store.save_agent_preferences(summary.id, &persisted)?;
            }
            self.publish(Update {
                activity: self.activity.label(),
                worktree_cwd: Some(cwd),
                status: "Connecting…".into(),
                ..Default::default()
            })?;
        }
        let settings = &self.input.settings;
        let working = settings
            .worktree_cwd
            .clone()
            .unwrap_or_else(|| summary.cwd.clone());
        if worktree_management::is_being_removed(&working) {
            return Err("This worktree is being removed.".into());
        }
        let setup_data = locked(&self.store)?.data_dir().map(Path::to_path_buf);
        let setup_notice = if let Some(data_dir) = setup_data
            .as_deref()
            .filter(|data_dir| worktree_setup::is_due(data_dir, Path::new(&working)))
        {
            self.publish(Update {
                activity: self.activity.label(),
                status: "Preparing worktree…".into(),
                ..Default::default()
            })?;
            let notice = process::owned_turn(summary.id, &self.cancel, || {
                worktree_setup::run_due(
                    data_dir,
                    Path::new(&summary.cwd),
                    Path::new(&working),
                    &self.cancel,
                )
            })
            .map_err(|error| format!("Could not prepare worktree setup: {error}"))?;
            if self.cancel.load(Ordering::Relaxed) {
                return Err("Stopped".into());
            }
            notice
        } else {
            None
        };
        Ok(setup_notice)
    }
    fn prepare_handoff(&mut self) -> Result<(), String> {
        let settings = &mut self.input.settings;
        let summary = &self.input.summary;
        if let Some(source) = settings.pending_provider_switch.take() {
            let root = locked(&self.store)?.data_dir().map(Path::to_path_buf);
            let prepared = handoff::prepare(
                TurnHistory {
                    summary,
                    earlier: &self.transcript.rows[..self.start],
                },
                &source,
                settings.worktree_cwd.as_deref().unwrap_or(&summary.cwd),
                &self.user_prompt,
                root,
                self.cancel.clone(),
            );
            locked(&self.store)?.prepare_local_provider_handoff(
                summary.id,
                &summary.provider,
                &self.account,
                &source,
                &prepared,
            )?;
            settings.handoff = Some(prepared.clone());
            self.handoff = Some(prepared.clone());
            self.provider_id = None;
            self.publish(Update {
                activity: self.activity.label(),
                handoff: Some(Some(prepared)),
                status: "Connecting…".into(),
                ..Default::default()
            })?;
        }
        Ok(())
    }
    fn prepare_request(
        &mut self,
        kind: Kind,
        binary: PathBuf,
        setup_notice: &mut Option<Notice>,
    ) -> Result<(Request, Option<Server>), String> {
        let settings = &mut self.input.settings;
        let summary = &self.input.summary;
        // These providers parse slash arguments themselves. App context before
        // the command would turn it into an ordinary message instead.
        let earlier = &self.transcript.rows[..self.start];
        let prompt = settings
            .prepared_prompt
            .take()
            .unwrap_or(std::mem::take(&mut self.input.prompt));
        let raw_native_command = kind == Kind::Omp
            && provider::native_prompt(kind, &prompt)
                .trim_start()
                .starts_with('/');
        *setup_notice = setup_notice.take().filter(|_| !raw_native_command);
        let prompt = if raw_native_command {
            prompt
        } else {
            turn_prompt(
                prompt,
                settings,
                TurnHistory { summary, earlier },
                self.provider_id.as_deref(),
                &mut self.bot_context,
                &self.store,
            )?
        };
        let cwd = settings.worktree_cwd.as_deref().unwrap_or(&summary.cwd);
        if settings.bot.is_none()
            && cwd != "~"
            && let Some(store) = checkpoint::store()
        {
            let id = checkpoint::new_turn_id();
            let session = summary.id.to_string();
            // Capture errors are persisted and shown in the review.
            let _ = store.exclusive(|store| {
                // An interrupted turn continues with this session's edit evidence.
                store.capture(&session, cwd, &edited_paths(earlier))?;
                store.begin_turn(&session, cwd, &id)
            });
            self.checkpoint = Some((cwd.to_owned(), id));
        }
        let mut prompt = provider::prepare_prompt(kind, &prompt, cwd, &settings.attachments)?;
        if !raw_native_command && self.user_prompt.contains("@note:") {
            prompt = locked(&self.store)?.apply_legacy_notes_to_turn(&self.user_prompt, prompt)?;
        }
        if !raw_native_command && settings.orchestrator {
            prompt = format!("{}\n\nUser request:\n{prompt}", orchestration::INSTRUCTIONS);
        }
        if !raw_native_command && settings.operator {
            let cli = std::env::current_exe().map_err(|e| e.to_string())?;
            prompt = format!(
                "You have vorkspace app access for this project. Use the CLI: {} app ACTION --json JSON. Supported actions: models.list {{}}, sessions.list {{}}, sessions.read {{sessionId}}, sessions.start {{prompt,harness?,model?,title?,draft?,reveal?}}, sessions.send/draft {{sessionId,prompt}}, sessions.stop {{sessionId}}, folders.list {{}}, folders.move {{sessionId,folderId?,newFolderName?}}, notes.list {{}}, notes.read {{id}}, notes.write {{id?,title,body}}, worktrees.list {{}}. Scope is this thread's project. App credentials are in the environment; never print them. Respect the user’s request and selected access policy.\n\n{prompt}",
                shlex_quote(&cli.to_string_lossy())
            );
        }
        if let Some(notice) = &*setup_notice {
            // Keep command output out of the composer and note token parsers.
            let warning = notice.warning();
            prompt = format!(
                "<vorkspace_context>\nVorkspace adds this context, not the user. {warning}\n</vorkspace_context>\n\n{prompt}"
            );
        }
        // Every foreground session receives an independent visual capability.
        // This happens after configuration was persisted, and Settings' env is
        // transport-only, so credentials never enter saved conversation settings.
        let visual_bridge = if !settings.text_only && !raw_native_command {
            Some(Server::start(summary.id)?)
        } else {
            None
        };
        if let Some(visual_bridge) = &visual_bridge {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            settings
                .environment
                .extend(visual_bridge.environment(&executable));
            self.discovery = visual_discovery::Discovery::prepare(
                &*locked(&self.store)?,
                summary.id,
                kind,
                &settings.account.id,
                self.provider_id.as_deref(),
                &executable,
            )?;
            if let Some(discovery) = &self.discovery {
                prompt = discovery.prepend(prompt);
            }
        }
        let request = Request {
            kind,
            binary,
            cwd: settings
                .worktree_cwd
                .as_deref()
                .unwrap_or(&summary.cwd)
                .into(),
            prompt,
            provider_id: self.provider_id.clone(),
            model: settings.model.clone(),
            settings: std::mem::take(settings),
        };
        Ok((request, visual_bridge))
    }

    fn settle_tools(&mut self, result: &Result<(), String>, cancelled: bool) -> usize {
        // A storage error or cancelled publish can leave this turn's latest rows
        // unseen by the UI. Replay the turn on failure before sending an error or
        // settled tools; starting after those missing rows would abort in patch().
        let mut final_start = if result.is_err() {
            self.start
        } else {
            self.transcript.rows.len()
        };
        for (ix, row) in self.transcript.rows.iter_mut().enumerate().skip(self.start) {
            if row.role != "tool" {
                continue;
            }
            if let Ok(mut tool) = serde_json::from_str::<ToolCall>(&row.text)
                && tool.state == ToolState::Running
            {
                tool.state = if cancelled {
                    ToolState::Cancelled
                } else {
                    ToolState::Failed
                };
                if tool.output.is_empty() {
                    tool.output = if cancelled {
                        "Cancelled".into()
                    } else {
                        result.as_ref().err().cloned().unwrap_or_else(|| {
                            "No completion result was received before the turn ended.".into()
                        })
                    };
                }
                row.text = serde_json::to_string(&tool).unwrap();
                final_start = final_start.min(ix);
            }
        }
        if let Some((cwd, id)) = &self.checkpoint
            && let Some(store) = checkpoint::store()
        {
            // The provider has exited, so this snapshot is the turn's settled result.
            let session = self.session.to_string();
            let _ = store.exclusive(|store| {
                store.capture_current(
                    &session,
                    cwd,
                    id,
                    &edited_paths(&self.transcript.rows[self.start..]),
                )?;
                store.finish_turn(&session, cwd, id)
            });
        }
        final_start
    }

    fn finish_history(
        &mut self,
        result: &Result<(), String>,
        cancelled: bool,
        final_start: usize,
        outcome: &mut TurnOutcome,
    ) -> (usize, String, Option<String>, Option<HistoryRecovery>) {
        let (mut start, mut status) = if self.pending.overlap {
            (0, String::new())
        } else if !self.history_loaded {
            // A rejected preflight has no row baseline. Writing an error at row 0
            // would replace a real conversation's first saved message.
            (
                0,
                if cancelled {
                    "Ready".into()
                } else {
                    result
                        .as_ref()
                        .err()
                        .cloned()
                        .unwrap_or_else(|| "Conversation was not loaded".into())
                },
            )
        } else if let Err(error) = result.as_ref()
            && !cancelled
        {
            let start = final_start.min(self.transcript.begin("error"));
            self.transcript.append(error);
            self.pending.changed(start);
            let error = match self.flush(Instant::now()) {
                Ok(()) => error.clone(),
                Err(storage) => format!("{error} · Saving failed: {storage}"),
            };
            (start, error)
        } else {
            if final_start < self.transcript.rows.len() {
                self.pending.changed(final_start);
            }
            let status = match self.flush(Instant::now()) {
                Ok(()) => "Ready".into(),
                Err(error) => {
                    *outcome = TurnOutcome::Failed;
                    format!("Saving failed: {error}")
                }
            };
            (final_start, status)
        };
        // Settle provenance in storage before notifying the view, including when
        // its window has closed or the provider was cancelled.
        let mut settled_provider_turn_id = None;
        if let Some((turn, mut info)) = self.turn_info.take().filter(|_| !self.pending.overlap) {
            let completed = vorkspace_core::time::now_ms();
            info.duration_ms = info.started_at.map_or(0, |at| completed.saturating_sub(at));
            info.duration_known = true;
            info.completed_at = info.started_at.map(|_| completed);
            settled_provider_turn_id = info.provider_turn_id.clone();
            if let Err(error) = self.pending.receipt(&self.store, self.session, turn, info) {
                *outcome = TurnOutcome::Failed;
                status = format!("{status} · Saving turn metadata failed: {error}");
            }
        }
        let mut history_recovery = None;
        if self.pending.overlap {
            *outcome = TurnOutcome::Failed;
            let recovery = locked(&self.store).and_then(|store| {
                store.save_or_retain_turn_overlap(
                    self.session,
                    &self.recovery_id,
                    &self.transcript.rows[self.start..],
                    self.provider_id
                        .as_deref()
                        .filter(|_| self.submissions.was_dispatched()),
                )
            });
            start = 0;
            match recovery {
                Ok(()) => {
                    history_recovery = Some(HistoryRecovery::Saved);
                    status = "Recovered a competing version of this turn".into();
                }
                Err(error) => {
                    // The store retains the complete payload until its retry
                    // commits, independently of the projected transcript.
                    history_recovery = Some(HistoryRecovery::Unsaved);
                    if self.bot.is_none() {
                        start = self.start;
                    }
                    status = format!("Could not save the recovered version: {error}");
                }
            }
        }
        (start, status, settled_provider_turn_id, history_recovery)
    }

    fn finish(mut self, result: Result<(), String>) {
        let cancelled = self.cancel.load(Ordering::Relaxed)
            || result.as_ref().is_err_and(|error| error == "Stopped");
        let mut outcome = if cancelled {
            TurnOutcome::Cancelled
        } else if result.is_ok() && self.history_loaded {
            TurnOutcome::Completed
        } else {
            TurnOutcome::Failed
        };
        let mut handoff = self.handoff_retired.then_some(None);
        let retained_handoff = self
            .handoff
            .as_ref()
            .filter(|_| !self.handoff_accepted)
            .map(|prepared| {
                let mut ready = prepared.clone();
                // This accepted user message never reached the incoming provider.
                // Carry it with the ready packet when the user continues later.
                if !self.user_prompt.trim().is_empty() {
                    ready.brief.push_str(
                        "\n\nAfter the switch, before this message, the user also sent:\n\n",
                    );
                    ready.brief.push_str(&self.user_prompt);
                }
                ready
            });
        let final_start = self.settle_tools(&result, cancelled);
        let (mut start, mut status, settled_provider_turn_id, history_recovery) =
            self.finish_history(&result, cancelled, final_start, &mut outcome);
        let mut submissions = self.submissions;
        let submission_save = submissions.finish(&result, self.provider_completed);
        drop(submissions);
        if let (Some(prepared), Some(ready)) = (self.handoff.as_ref(), retained_handoff)
            && !self.pending.overlap
        {
            match locked(&self.store).and_then(|store| {
                store.settle_local_provider_handoff(
                    self.session,
                    &self.input.summary.provider,
                    &self.account,
                    prepared,
                    Some(&ready),
                )
            }) {
                Ok(true) => handoff = Some(Some(ready)),
                Ok(false) => (),
                Err(error) => {
                    outcome = TurnOutcome::Failed;
                    status = format!("{status} · Saving handoff failed: {error}");
                }
            }
        }
        if let Err(error) = submission_save {
            outcome = TurnOutcome::Failed;
            status = format!("{status} · Saving local send receipt failed: {error}");
        }
        let settlement = TurnSettlement {
            outcome,
            history_recovery,
            usage_limit: if cancelled {
                None
            } else {
                self.usage_limit.take()
            },
        };
        let bot_snapshot = self.history_loaded && !self.pending.overlap && self.bot.is_some();
        let ordinary_unsaved_recovery =
            self.bot.is_none() && settlement.history_recovery == Some(HistoryRecovery::Unsaved);
        if let Some(bot) = self.bot.filter(|_| bot_snapshot) {
            // Publish the pointer from durable state after the turn settles. A
            // delayed provider event must not restore an old account or session.
            let saved = (|| {
                let store = locked(&self.store)?;
                let Some(session) = store.bot_session(self.session)? else {
                    return Ok(());
                };
                let settings = store.agent_preferences(self.session)?.unwrap_or_default();
                if session.provider == self.input.summary.provider
                    && settings.account.id == self.account
                {
                    bot::home::refresh_session(&bot.root, &bot.bot, &session, &settings.account)?;
                }
                Ok::<_, String>(())
            })();
            if let Err(error) = saved {
                // This is a convenience pointer, not a turn receipt. Marking the
                // turn failed here would requeue work the provider already did.
                eprintln!("Could not update Bot SESSION.md: {error}");
            }
        }
        // Stop can discard the initial snapshot even after the worker enqueues it.
        // Send the whole admitted Bot transcript at settlement. Durable writes above
        // still start at the changed suffix; the final snapshot takes the owned rows.
        if bot_snapshot {
            start = 0;
        }
        let rows = if !self.history_loaded || (self.pending.overlap && !ordinary_unsaved_recovery) {
            Vec::new()
        } else if start == 0 {
            self.transcript.rows
        } else {
            self.transcript.rows.split_off(start)
        };
        // Final status must still arrive after cancellation. Dropping a window closes
        // the receiver, so the worker never keeps a closed viewport alive.
        let _ = self.tx.send_blocking(Update {
            activity: self.activity.label(),
            // A stop can make the UI skip intermediate updates. Keep the created
            // checkout in the final receipt so the next Send cannot create it twice.
            worktree_cwd: self.created_worktree,
            follow_up_delivered: self.accepted_ids,
            delivered: self.delivered,
            transcript: if bot_snapshot {
                TranscriptPatch::BotSnapshot {
                    turn_start: self.start,
                    rows,
                }
            } else {
                TranscriptPatch::tail(start, rows)
            },
            provider_turn_id: settled_provider_turn_id.filter(|_| bot_snapshot),
            binding: if self.pending.overlap {
                ProviderBinding::Unchanged
            } else {
                self.binding
            },
            configuration: self.configuration.filter(|_| !self.pending.overlap),
            context: (self.provider_reset && !self.pending.overlap).then_some(self.context),
            transition: TurnTransition::Settled(settlement),
            handoff: if self.pending.overlap { None } else { handoff },
            status,
            ..Default::default()
        });
    }
}
pub(super) fn run_turn_using(
    input: TurnInput,
    controls: std::sync::mpsc::Receiver<Control>,
    store: Database,
    cancel: Arc<AtomicBool>,
    tx: async_channel::Sender<Update>,
    resolve_binary: impl FnOnce(Kind) -> Option<PathBuf>,
) {
    let mut turn = RunningTurn::new(input, store, cancel, tx);
    let result = (|| {
        let kind = turn.prepare_history()?;
        let creating_worktree = turn.save_prompt()?;
        let mut setup_notice = turn.prepare_worktree(creating_worktree)?;
        turn.prepare_handoff()?;
        let binary = resolve_binary(kind).ok_or_else(|| {
            format!(
                "{} CLI not found. Install it and sign in, or set VORKSPACE_{}_BINARY.",
                kind.label(),
                kind.id().to_uppercase()
            )
        })?;
        let (request, visual_bridge) = turn.prepare_request(kind, binary, &mut setup_notice)?;
        let activity_cwd = request.cwd.to_string_lossy().into_owned();
        if turn.cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        turn.submissions
            .dispatch_initial(turn.input.composer_origin.as_ref())?;
        if let Some(origin) = turn.input.composer_origin.take() {
            // The transfer has already committed. Deliver it even if Stop is
            // concurrently draining progress updates.
            turn.tx
                .send_blocking(Update {
                    transition: TurnTransition::AcceptedComposer(origin),
                    activity: turn.activity.label(),
                    transcript: TranscriptPatch::tail(
                        turn.start,
                        turn.transcript.rows[turn.start..].to_vec(),
                    ),
                    status: "Connecting…".into(),
                    ..Default::default()
                })
                .map_err(|_| "Window closed".to_owned())?;
        }
        let cancel = turn.cancel.clone();
        // The running-work list attributes the provider and its tools to this session.
        let result = process::owned_turn(turn.session, &cancel, || {
            provider::run_with_controls(request, cancel.clone(), Some(controls), |events| {
                turn.events(events, kind, &activity_cwd, visual_bridge.as_ref())
            })
        });
        if result.is_ok() {
            turn.provider_completed = true;
        }
        // Provider readiness has different meanings across transports. A
        // confirmed turn proves the prompt was carried, even if the UI closed.
        if turn.provider_completed
            && let Some(notice) = setup_notice
        {
            let _ = notice.acknowledge();
        }
        if result.is_ok()
            && let Some(receipt) = &turn.bot_context
        {
            locked(&turn.store)?.save_setting(
                &format!("bot-context:{}", turn.session),
                &receipt.encode(turn.provider_id.as_deref()),
            )?;
        }
        if result.is_ok()
            && !turn.cancel.load(Ordering::Relaxed)
            && let Some(discovery) = &turn.discovery
        {
            discovery.remember(&*locked(&turn.store)?, turn.provider_id.as_deref())?;
        }
        result
    })();
    turn.finish(result);
}
