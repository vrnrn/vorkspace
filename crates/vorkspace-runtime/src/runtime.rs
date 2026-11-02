use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use vorkspace_core::agent::{Approval, Control, Settings};
use vorkspace_core::{
    provider::{self, Event, Kind, Request},
    store::{Session, Store},
    transcript::{Row, Transcript},
};
mod activity;
pub mod card_persistence;
#[cfg(test)]
mod follow_up_tests;
mod handoff;
mod persistence;
pub mod remote;
mod submissions;
#[cfg(test)]
mod ticket_context_tests;
mod visual_discovery;
#[cfg(test)]
mod visual_tests;
use persistence::PendingSave;
pub type Database = Arc<Mutex<Store>>;
struct TurnInput {
    summary: Session,
    prompt: String,
    settings: Settings,
}
#[derive(Default)]
pub struct Update {
    /// Full, bounded latest-tool label for this run; None means no known tool.
    pub activity: Option<String>,
    /// Present only when rows describe the complete authoritative snapshot tail.
    pub source_len: Option<usize>,
    pub worktree_cwd: Option<String>,
    pub commands: Option<Vec<provider::NativeCommand>>,
    pub provider_turn_id: Option<String>,
    pub turn_ready: bool,
    pub follow_up_delivered: Vec<String>,
    pub follow_up_errors: Vec<(String, String)>,
    pub delivered: Option<String>,
    pub start: usize,
    pub rows: Vec<Row>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub config_model: Option<String>,
    pub config_settings: Option<std::collections::HashMap<String, String>>,
    pub context: Option<vorkspace_core::context::Usage>,
    pub metrics: Option<vorkspace_core::agent::TurnMetrics>,
    pub usage_limit: Option<vorkspace_core::usage::Limit>,
    pub approvals: Vec<Approval>,
    pub approvals_resolved: Vec<String>,
    pub questions: Vec<vorkspace_core::agent::Question>,
    pub questions_resolved: Vec<String>,
    pub handoff: Option<Option<vorkspace_core::agent::Handoff>>,
    pub status: String,
    pub finished: bool,
}
fn publish(
    tx: &async_channel::Sender<Update>,
    cancel: &AtomicBool,
    mut update: Update,
) -> Result<(), String> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        match tx.try_send(update) {
            Ok(()) => return Ok(()),
            Err(async_channel::TrySendError::Closed(_)) => return Err("Window closed".into()),
            Err(async_channel::TrySendError::Full(value)) => update = value,
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}
pub fn locked(store: &Database) -> Result<std::sync::MutexGuard<'_, Store>, String> {
    store
        .lock()
        .map_err(|_| "Native session store unavailable".into())
}

pub fn run_turn(
    summary: Session,
    prompt: String,
    settings: Settings,
    controls: std::sync::mpsc::Receiver<Control>,
    store: Database,
    cancel: Arc<AtomicBool>,
    tx: async_channel::Sender<Update>,
) {
    if summary.cwd.starts_with("remote://") {
        remote::run(summary, prompt, settings, controls, store, cancel, tx);
        return;
    }
    run_turn_using(
        TurnInput {
            summary,
            prompt,
            settings,
        },
        controls,
        store,
        cancel,
        tx,
        provider::resolve,
    );
}

fn run_turn_using(
    input: TurnInput,
    controls: std::sync::mpsc::Receiver<Control>,
    store: Database,
    cancel: Arc<AtomicBool>,
    tx: async_channel::Sender<Update>,
    resolve_binary: impl FnOnce(Kind) -> Option<std::path::PathBuf>,
) {
    let TurnInput {
        summary,
        prompt,
        mut settings,
    } = input;
    if settings.account.root.is_none() && settings.account.id != vorkspace_core::accounts::DEFAULT {
        settings.account.root = locked(&store)
            .ok()
            .and_then(|s| s.data_dir().map(std::path::Path::to_path_buf));
    }
    let source_account = settings.account.id.clone();
    let user_request = prompt.clone();
    let mut transcript = Transcript::default();
    let mut history_loaded = false;
    let shared = locked(&store)
        .map(|s| s.uses_vorkspace_schema())
        .unwrap_or(false);
    let mut pending = PendingSave::new(shared, std::time::Instant::now());
    let mut submissions = submissions::Submissions::new(store.clone(), summary.id);
    let mut provider_completion_confirmed = false;
    pending.own_context(&summary.provider, &source_account);
    let mut initial_delivery = None;
    let mut accepted_messages = Vec::new();
    let mut provider_id = summary.provider_id.clone();
    let mut turn_start = 0;
    let mut activity = activity::ToolActivity::default();
    let mut turn_receipt: Option<(usize, vorkspace_core::agent::TurnInfo)> = None;
    let mut bot_fingerprint = None;
    let mut visual_introduction = None;
    let mut prepared_handoff = None;
    let mut handoff_accepted = false;
    // SessionReview's recorded turn: its project and id, once the baseline is taken.
    let mut checkpoint: Option<(String, String)> = None;
    let mut created_worktree = None;
    let mut setup_warning = None;
    let mut context = locked(&store)
        .ok()
        .and_then(|s| s.context_usage(summary.id).ok().flatten())
        .unwrap_or_default();
    let mut usage_limit = None;
    let result = (|| {
        let kind = Kind::parse(&summary.provider).ok_or_else(|| {
            format!(
                "Provider {} is no longer supported; its saved history is still available",
                summary.provider
            )
        })?;
        if let Some(reason) = locked(&store)?.edit_recovery_reason(summary.id)? {
            return Err(reason);
        }
        if settings.worktree_removed {
            return Err("Select a branch or worktree to continue this conversation.".into());
        }
        if settings.account.id != vorkspace_core::accounts::DEFAULT
            && !locked(&store)?
                .provider_accounts()?
                .exists(Kind::from_id(&summary.provider), &settings.account.id)
        {
            return Err(
                "This provider account was removed. Choose another account to continue.".into(),
            );
        }
        transcript = locked(&store)?.load(summary.id)?;
        history_loaded = true;
        turn_start = transcript.rows.len();
        settings.ticket_context_data_dir =
            locked(&store)?.data_dir().map(std::path::Path::to_path_buf);
        settings.ticket_context = Some(vorkspace_core::ticket_context::admit(
            &prompt,
            settings.ticket_context_data_dir.as_deref(),
            std::path::Path::new(settings.worktree_cwd.as_deref().unwrap_or(&summary.cwd)),
            &cancel,
            settings.ticket_context.take(),
        )?);
        submissions.reserve_initial(&prompt, &settings)?;
        let mut persisted = settings.clone();
        persisted.handoff = None;
        // These modes belong to this turn, not the next composer draft.
        persisted.plan = false;
        persisted.operator = false;
        persisted.orchestrator = false;
        let start = transcript.begin(if settings.internal_message {
            "bot-system"
        } else {
            "user"
        });
        if let Some(at) = settings
            .turn_model
            .as_ref()
            .and_then(|turn| turn.started_at)
        {
            transcript.rows[start].sent_at = Some(at);
        }
        if settings.bot.is_some()
            && let Some(at) = settings.bot_sent_at
        {
            transcript.rows[start].sent_at = Some(at);
        }
        let started_at = transcript.rows[start].sent_at;
        transcript.append(&prompt);
        transcript.attach_files(&settings.attachments);
        // Show the working receipt while connecting, before the first token.
        transcript.begin("assistant");
        // Keep the already painted prompt in the failure replay even if saving
        // the turn's configuration fails before its first transcript write.
        locked(&store)?.configure_agent(summary.id, &summary.provider, &persisted)?;
        locked(&store)?.write_rows_with_message(
            summary.id,
            start,
            &transcript.rows[start..],
            settings.bot_message_id.as_deref(),
        )?;
        let mut info =
            settings
                .turn_model
                .clone()
                .unwrap_or_else(|| vorkspace_core::agent::TurnInfo {
                    provider: summary.provider.clone(),
                    model_id: settings.model.clone(),
                    ..Default::default()
                });
        info.started_at = info.started_at.or(started_at);
        // The receipt's key follows all continuation rows of the prompt.
        let turn = transcript
            .rows
            .iter()
            .rposition(Row::is_prompt)
            .map_or(0, |at| at + 1);
        persistence::save_turn_receipt(&store, summary.id, turn, info.clone())?;
        turn_receipt = Some((turn, info));
        initial_delivery = settings.bot_message_id.clone();
        let creating_worktree = settings.new_worktree && settings.worktree_cwd.is_none();
        publish(
            &tx,
            &cancel,
            Update {
                activity: activity.label(),
                delivered: settings.bot_message_id.clone(),
                start,
                rows: transcript.rows[start..].to_vec(),
                status: if creating_worktree {
                    "Creating worktree…"
                } else if settings.pending_provider_switch.is_some() {
                    "Preparing handoff…"
                } else {
                    "Connecting…"
                }
                .into(),
                ..Default::default()
            },
        )?;
        if creating_worktree {
            let path = vorkspace_core::git::create_worktree(
                std::path::Path::new(&summary.cwd),
                settings.worktree_base.as_deref().unwrap_or("HEAD"),
                summary.id,
            )?;
            let cwd = path.to_string_lossy().into_owned();
            created_worktree = Some(cwd.clone());
            settings.worktree_cwd = Some(cwd.clone());
            settings.new_worktree = false;
            {
                let store = locked(&store)?;
                let mut persisted = store.agent_settings(summary.id)?.unwrap_or_default();
                persisted.worktree_cwd = Some(cwd.clone());
                persisted.new_worktree = false;
                store.save_agent_settings(summary.id, &persisted)?;
            }
            publish(
                &tx,
                &cancel,
                Update {
                    activity: activity.label(),
                    worktree_cwd: Some(cwd),
                    start: transcript.rows.len(),
                    status: "Connecting…".into(),
                    ..Default::default()
                },
            )?;
        }
        let working = settings
            .worktree_cwd
            .clone()
            .unwrap_or_else(|| summary.cwd.clone());
        if vorkspace_core::worktree_management::is_being_removed(&working) {
            return Err("This worktree is being removed.".into());
        }
        if vorkspace_core::worktree_setup::is_due(std::path::Path::new(&working)) {
            publish(
                &tx,
                &cancel,
                Update {
                    activity: activity.label(),
                    start: transcript.rows.len(),
                    status: "Preparing worktree…".into(),
                    ..Default::default()
                },
            )?;
            setup_warning = vorkspace_core::worktree_setup::run_due(
                std::path::Path::new(&summary.cwd),
                std::path::Path::new(&working),
                &cancel,
            );
            if cancel.load(Ordering::Relaxed) {
                return Err("Stopped".into());
            }
        }
        if let Some(source) = settings.pending_provider_switch.take() {
            let root = locked(&store)?.data_dir().map(std::path::Path::to_path_buf);
            let prepared = handoff::prepare(
                summary.id,
                &source,
                settings.worktree_cwd.as_deref().unwrap_or(&summary.cwd),
                &user_request,
                &transcript.rows[..turn_start],
                root,
                cancel.clone(),
            );
            settings.handoff = Some(prepared.clone());
            prepared_handoff = Some(prepared.clone());
            provider_id = None;
            {
                let store = locked(&store)?;
                store.reset_provider_id(summary.id)?;
                let mut persisted = store.agent_settings(summary.id)?.unwrap_or_default();
                persisted.pending_provider_switch = None;
                persisted.handoff = Some(prepared.clone());
                store.save_agent_settings(summary.id, &persisted)?;
            }
            publish(
                &tx,
                &cancel,
                Update {
                    activity: activity.label(),
                    start: transcript.rows.len(),
                    handoff: Some(Some(prepared)),
                    status: "Connecting…".into(),
                    ..Default::default()
                },
            )?;
        }
        let binary = resolve_binary(kind).ok_or_else(|| {
            format!(
                "{} CLI not found. Install it and sign in, or set VORKSPACE_{}_BINARY.",
                kind.label(),
                kind.id().to_uppercase()
            )
        })?;
        // These providers parse slash arguments themselves. App context before
        // the command would turn it into an ordinary message instead.
        let prompt = settings.prepared_prompt.take().unwrap_or(prompt);
        let raw_native_command = kind == Kind::Omp
            && provider::native_prompt(kind, &prompt)
                .trim_start()
                .starts_with('/');
        let prompt = if raw_native_command {
            prompt
        } else {
            turn_prompt(
                prompt,
                &settings,
                TurnHistory {
                    summary: &summary,
                    earlier: &transcript.rows[..turn_start],
                    context: &context,
                },
                &mut provider_id,
                &mut bot_fingerprint,
                &store,
            )?
        };
        // A failed setup leaves a usable worktree, so the agent hears about it and
        // can repair it instead of the turn stopping.
        let prompt = match setup_warning.take() {
            Some(warning) => format!(
                "<vorkspace_context>\nVorkspace adds this context, not the user. {warning}\n</vorkspace_context>\n\n{prompt}"
            ),
            None => prompt,
        };
        let cwd = settings.worktree_cwd.as_deref().unwrap_or(&summary.cwd);
        if settings.bot.is_none()
            && cwd != "~"
            && let Some(store) = vorkspace_core::checkpoint::store()
        {
            let id = vorkspace_core::checkpoint::new_turn_id();
            let session = summary.id.to_string();
            // Capture errors are persisted and shown in the review.
            let _ = store.exclusive(|store| {
                // An interrupted turn continues with this session's edit evidence.
                store.capture(&session, cwd, &edited_paths(&transcript.rows[..turn_start]))?;
                store.begin_turn(&session, cwd, &id)
            });
            checkpoint = Some((cwd.to_owned(), id));
        }
        let mut prompt = provider::prepare_prompt(kind, &prompt, cwd, &settings.attachments)?;
        if !raw_native_command && prompt.contains("@note:") {
            let notes = locked(&store)?.notes()?;
            for note in notes {
                let token = format!("@note:{}", note.id);
                if prompt
                    .lines()
                    .filter(|line| !line.trim_start().starts_with('>'))
                    .flat_map(str::split_whitespace)
                    .any(|word| word.trim_end_matches([',', '.', ';']) == token)
                {
                    prompt = format!(
                        "Referenced note: {}\n<note>\n{}\n</note>\n\n{prompt}",
                        note.title,
                        vorkspace_core::response::limit(&note.body, 64_000)
                    );
                }
            }
        }
        if !raw_native_command && settings.orchestrator {
            prompt = format!(
                "{}\n\nUser request:\n{prompt}",
                vorkspace_core::orchestration::INSTRUCTIONS
            );
        }
        if !raw_native_command && settings.operator {
            let cli = std::env::current_exe().map_err(|e| e.to_string())?;
            prompt = format!(
                "You have vorkspace app access for this project. Use the CLI: {} app ACTION --json JSON. Supported actions: models.list {{}}, sessions.list {{}}, sessions.read {{sessionId}}, sessions.start {{prompt,harness?,model?,title?,draft?,reveal?}}, sessions.send/draft {{sessionId,prompt}}, sessions.stop {{sessionId}}, folders.list {{}}, folders.move {{sessionId,folderId?,newFolderName?}}, notes.list {{}}, notes.read {{id}}, notes.write {{id?,title,body}}, worktrees.list {{}}. Scope is this thread's project. App credentials are in the environment; never print them. Respect the user’s request and selected access policy.\n\n{prompt}",
                shlex_quote(&cli.to_string_lossy())
            );
        }
        // Every foreground session receives an independent visual capability.
        // This happens after configuration was persisted, and Settings' env is
        // transport-only, so credentials never enter saved conversation settings.
        let visual_bridge = if !settings.text_only && !raw_native_command {
            Some(vorkspace_core::visual_bridge::Server::start(summary.id)?)
        } else {
            None
        };
        if let Some(visual_bridge) = &visual_bridge {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            settings
                .environment
                .extend(visual_bridge.environment(&executable));
            visual_introduction = visual_discovery::Discovery::prepare(
                &*locked(&store)?,
                summary.id,
                kind,
                &settings.account.id,
                provider_id.as_deref(),
                &executable,
            )?;
            if let Some(discovery) = &visual_introduction {
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
            provider_id: provider_id.clone(),
            model: settings.model.clone(),
            settings,
        };
        let activity_cwd = request.cwd.to_string_lossy().into_owned();
        let mut tool_rows = std::collections::HashMap::new();
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        submissions.dispatch_initial()?;
        // The running-work list attributes the provider and its tools to this session.
        let result = vorkspace_core::process::owned_by(summary.id, || {
            provider::run_with_controls(request, cancel.clone(), Some(controls), |events| {
                // Cancellation freezes the transcript, even while the native
                // provider drains its interrupt acknowledgement. A provider
                // completion already received in this batch must still be
                // durably recorded if Stop races with its final publication.
                let completion_already_received =
                    events.iter().any(|event| matches!(event, Event::Done));
                if cancel.load(Ordering::Relaxed) && !completion_already_received {
                    return Err("Stopped".into());
                }
                let publications = visual_bridge
                    .as_ref()
                    .map(|bridge| bridge.drain())
                    .unwrap_or_default();
                if events.is_empty() && publications.is_empty() {
                    return Ok(());
                }
                let mut start = transcript.rows.len();
                let mut status = "Streaming…".to_string();
                let mut changed_id = None;
                let mut model = None;
                let mut config_model = None;
                let mut config_settings = None;
                let mut context_changed = false;
                let mut metrics = None;
                let mut approvals = Vec::new();
                let mut approvals_resolved = Vec::new();
                let mut questions = Vec::new();
                let mut questions_resolved = Vec::new();
                let mut handoff = None;
                let mut turn_ready = false;
                let mut completed_batch = false;
                let mut follow_up_delivered = Vec::new();
                let mut follow_up_errors = Vec::new();
                let mut boundary = false;
                let mut commands = None;
                let mut provider_turn_id = None;
                for event in events {
                    match event {
                        Event::Commands(discovered) => commands = Some(discovered),
                        Event::TurnId(id) => {
                            if let Some((_, info)) = &mut turn_receipt {
                                info.provider_turn_id = Some(id.clone());
                            }
                            provider_turn_id = Some(id);
                        }
                        Event::Ready => {
                            turn_ready = true;
                            boundary = true;
                            if prepared_handoff.is_some() && !handoff_accepted {
                                handoff_accepted = true;
                                handoff = Some(None);
                                let store = locked(&store)?;
                                let mut persisted =
                                    store.agent_settings(summary.id)?.unwrap_or_default();
                                persisted.handoff = None;
                                store.save_agent_settings(summary.id, &persisted)?;
                            }
                        }
                        Event::FollowUpDispatch(message) => {
                            submissions.dispatch_follow_up(&message)?;
                        }
                        Event::FollowUp(message) => {
                            let at = transcript.begin("user");
                            activity.prompt(at);
                            start = start.min(at);
                            transcript.append(&message.text);
                            transcript.attach_files(&message.attachments);
                            // Paint the working receipt with the accepted steer,
                            // even when the provider has no new output yet.
                            let turn = transcript.begin("assistant");
                            pending.accept(
                                &store,
                                summary.id,
                                at,
                                &transcript,
                                &message.id,
                                std::time::Instant::now(),
                            )?;
                            // The durable queue/transcript handoff owns acceptance.
                            // Keep its ID even if a later metadata write fails.
                            accepted_messages.push(message.id.clone());
                            submissions.accept_follow_up(&message.id)?;
                            if let Some((_, root)) = &turn_receipt {
                                // A steer keeps its send time and model identity,
                                // while timing/metrics belong to the original run.
                                let steer = vorkspace_core::agent::TurnInfo {
                                    provider: root.provider.clone(),
                                    model: root.model.clone(),
                                    model_id: root.model_id.clone(),
                                    provider_turn_id: root.provider_turn_id.clone(),
                                    ..Default::default()
                                };
                                persistence::save_turn_receipt(&store, summary.id, turn, steer)?;
                            }
                            follow_up_delivered.push(message.id);
                        }
                        Event::FollowUpFailed { id, error } => {
                            submissions.fail_follow_up(&id, &error)?;
                            follow_up_errors.push((id, error));
                        }
                        Event::BeginAssistant => {
                            if transcript
                                .rows
                                .last()
                                .is_none_or(|r| r.role != "assistant" || !r.text.is_empty())
                            {
                                let at = transcript.begin("assistant");
                                start = start.min(at);
                                pending.changed(at);
                            }
                        }
                        Event::Text(text) => {
                            if transcript.rows.last().is_none_or(|r| r.role != "assistant") {
                                start = start.min(transcript.begin("assistant"));
                            }
                            let at = transcript.append(&text);
                            start = start.min(at);
                            pending.changed(at);
                        }
                        Event::Reasoning(text) => {
                            if text.is_empty() {
                                continue;
                            }
                            if transcript.rows.last().is_none_or(|r| r.role != "reasoning") {
                                start = start.min(transcript.begin("reasoning"));
                            }
                            let at = transcript.append(&text);
                            start = start.min(at);
                            pending.changed(at);
                        }
                        Event::Tool(tool) => {
                            if tool.is_compaction() {
                                status =
                                    if tool.state == vorkspace_core::tool::ToolState::Running {
                                        "Compacting context…"
                                    } else {
                                        "Working…"
                                    }
                                    .into();
                            }
                            let ix = *tool_rows
                                .entry(tool.id.clone())
                                .or_insert_with(|| transcript.begin("tool"));
                            activity.tool(ix, &tool, &activity_cwd);
                            transcript.rows[ix].text =
                                serde_json::to_string(&tool).map_err(|e| e.to_string())?;
                            start = start.min(ix);
                            pending.changed(ix);
                        }
                        Event::SessionId(id) => {
                            if provider_id.as_ref() != Some(&id) {
                                let store = locked(&store)?;
                                let mut settings =
                                    store.agent_settings(summary.id)?.unwrap_or_default();
                                if let Some(pending) = settings.pending_provider_switch.as_mut()
                                    && pending.from == summary.provider
                                    && pending.account.id == source_account
                                {
                                    pending.provider_id = Some(id.clone());
                                    store.save_agent_settings(summary.id, &settings)?;
                                } else if settings.account.id == source_account
                                    && store.sessions()?.iter().any(|session| {
                                        session.id == summary.id
                                            && session.provider == summary.provider
                                    })
                                {
                                    store.provider_id(summary.id, &id)?;
                                }
                                provider_id = Some(id.clone());
                                changed_id = Some(id);
                            }
                        }
                        Event::Model(id) => model = Some(id),
                        Event::ConfigChanged {
                            model: changed_model,
                            model_settings,
                        } => {
                            let store = locked(&store)?;
                            let mut settings =
                                store.agent_settings(summary.id)?.unwrap_or_default();
                            if let Some(pending) = settings.pending_provider_switch.as_mut()
                                && pending.from == summary.provider
                                && pending.account.id == source_account
                            {
                                if let Some(id) = &changed_model {
                                    pending.model = Some(id.clone());
                                }
                                pending.model_settings.extend(model_settings.clone());
                            } else if settings.account.id == source_account
                                && store.sessions()?.iter().any(|session| {
                                    session.id == summary.id && session.provider == summary.provider
                                })
                            {
                                if let Some(id) = &changed_model {
                                    settings.model = Some(id.clone());
                                }
                                settings.model_settings.extend(model_settings.clone());
                            }
                            store.save_agent_settings(summary.id, &settings)?;
                            if changed_model.is_some() {
                                model = changed_model.clone();
                                config_model = changed_model;
                            }
                            config_settings
                                .get_or_insert_with(std::collections::HashMap::new)
                                .extend(model_settings);
                        }
                        Event::Interjection {
                            text,
                            custom_type,
                            severity,
                        } => {
                            let note = vorkspace_core::transcript::Interjection {
                                text,
                                custom_type,
                                severity,
                            };
                            let at = transcript.begin("interjection");
                            transcript
                                .append(&serde_json::to_string(&note).map_err(|e| e.to_string())?);
                            start = start.min(at);
                            pending.changed(at);
                            boundary = true;
                        }
                        Event::Metrics(reading) => {
                            if let Some((_, info)) = &mut turn_receipt {
                                info.metrics = Some(reading);
                            }
                            metrics = Some(reading);
                        }
                        Event::Context { used, window } => {
                            context.merge(used, window);
                            context_changed = true;
                            pending.context_changed();
                        }
                        Event::Approval(approval) => {
                            status = "Waiting for approval".into();
                            approvals.push(approval);
                            boundary = true;
                        }
                        Event::ApprovalResolved(id) => {
                            approvals_resolved.push(id);
                            boundary = true;
                        }
                        Event::Question(question) => {
                            status = "Waiting for your answer".into();
                            questions.push(question);
                            boundary = true;
                        }
                        Event::QuestionResolved(id) => {
                            questions_resolved.push(id);
                            boundary = true;
                        }
                        Event::Status(text) => status = text,
                        Event::UsageLimited(limit) => usage_limit = Some(limit),
                        Event::Done => {
                            completed_batch = true;
                            status = "Ready".into();
                            boundary = true;
                        }
                    }
                }
                for publication in &publications {
                    let document =
                        serde_json::to_string(&publication.document).map_err(|e| e.to_string())?;
                    let at = transcript.begin("visual");
                    transcript.append(&document);
                    start = start.min(at);
                    pending.changed(at);
                    boundary = true;
                }
                let now = std::time::Instant::now();
                let publication_result = (|| {
                    if pending.due(boundary, now) {
                        pending.flush(&store, summary.id, &transcript, &context, now)?;
                    }
                    // The provider's terminal signal is authoritative once its
                    // accompanying transcript changes are durable. A later UI
                    // publication failure must not turn this send uncertain.
                    if completed_batch {
                        provider_completion_confirmed = true;
                    }
                    publish(
                        &tx,
                        &cancel,
                        Update {
                            source_len: None,
                            activity: activity.label(),
                            worktree_cwd: None,
                            commands,
                            provider_turn_id,
                            turn_ready,
                            follow_up_delivered,
                            follow_up_errors,
                            delivered: None,
                            start,
                            rows: transcript.rows[start..].to_vec(),
                            provider_id: changed_id,
                            model,
                            config_model,
                            config_settings,
                            context: context_changed.then_some(context),
                            metrics,
                            usage_limit: None,
                            approvals,
                            approvals_resolved,
                            questions,
                            questions_resolved,
                            handoff,
                            status,
                            finished: false,
                        },
                    )
                })();
                // A tool succeeds only after both durable storage and the live
                // transcript update have accepted the visual.
                for publication in publications {
                    let result = publication_result.as_ref().map(|_| {
                        serde_json::json!({"id":publication.document.id,"title":publication.document.title,"saved":true})
                    }).map_err(Clone::clone);
                    let _ = publication.reply.send(result);
                }
                publication_result
            })
        });
        if result.is_ok() {
            provider_completion_confirmed = true;
        }
        if result.is_ok()
            && let Some(fingerprint) = &bot_fingerprint
        {
            locked(&store)?.save_setting(
                &format!("bot-context:{}", summary.id),
                &serde_json::json!({"provider":provider_id,"hash":fingerprint}).to_string(),
            )?;
        }
        if result.is_ok()
            && !cancel.load(Ordering::Relaxed)
            && let Some(discovery) = &visual_introduction
        {
            discovery.remember(&*locked(&store)?, provider_id.as_deref())?;
        }
        result
    })();
    let submission_save = submissions.finish(&result, provider_completion_confirmed);
    drop(submissions);
    let cancelled =
        cancel.load(Ordering::Relaxed) || result.as_ref().is_err_and(|error| error == "Stopped");
    let handoff = prepared_handoff.map(|mut ready| {
        if handoff_accepted {
            None
        } else {
            // This accepted user message never reached the incoming provider.
            // Carry it with the ready packet when the user continues later.
            if !user_request.trim().is_empty() {
                ready
                    .brief
                    .push_str("\n\nAfter the switch, before this message, the user also sent:\n\n");
                ready.brief.push_str(&user_request);
            }
            Some(ready)
        }
    });
    let handoff_save = if let Some(ready) = &handoff {
        locked(&store).and_then(|store| {
            let mut persisted = store.agent_settings(summary.id)?.unwrap_or_default();
            persisted.pending_provider_switch = None;
            persisted.handoff = ready.clone();
            store.save_agent_settings(summary.id, &persisted)
        })
    } else {
        Ok(())
    };
    // A storage error or cancelled publish can leave this turn's latest rows
    // unseen by the UI. Replay the turn on failure before sending an error or
    // settled tools; starting after those missing rows would abort in patch().
    let mut final_start = if result.is_err() {
        turn_start
    } else {
        transcript.rows.len()
    };
    for (ix, row) in transcript.rows.iter_mut().enumerate().skip(turn_start) {
        if row.role != "tool" {
            continue;
        }
        if let Ok(mut tool) = serde_json::from_str::<vorkspace_core::tool::ToolCall>(&row.text)
            && tool.state == vorkspace_core::tool::ToolState::Running
        {
            tool.state = if cancelled {
                vorkspace_core::tool::ToolState::Cancelled
            } else {
                vorkspace_core::tool::ToolState::Failed
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
    if let Some((cwd, id)) = &checkpoint
        && let Some(store) = vorkspace_core::checkpoint::store()
    {
        // The provider has exited, so this snapshot is the turn's settled result.
        let session = summary.id.to_string();
        let _ = store.exclusive(|store| {
            store.capture(&session, cwd, &edited_paths(&transcript.rows[turn_start..]))?;
            store.finish_turn(&session, cwd, id)
        });
    }
    let (start, rows, mut status) = if !history_loaded {
        // A rejected preflight has no row baseline. Writing an error at row 0
        // would replace a real conversation's first saved message.
        (
            0,
            Vec::new(),
            if cancelled {
                "Ready".into()
            } else {
                result
                    .err()
                    .unwrap_or_else(|| "Conversation was not loaded".into())
            },
        )
    } else if let Err(error) = result
        && !cancelled
    {
        let start = final_start.min(transcript.begin("error"));
        transcript.append(&error);
        pending.changed(start);
        let error = match pending.flush(
            &store,
            summary.id,
            &transcript,
            &context,
            std::time::Instant::now(),
        ) {
            Ok(()) => error,
            Err(storage) => format!("{error} · Saving failed: {storage}"),
        };
        (start, transcript.rows[start..].to_vec(), error)
    } else {
        if final_start < transcript.rows.len() {
            pending.changed(final_start);
        }
        let status = pending
            .flush(
                &store,
                summary.id,
                &transcript,
                &context,
                std::time::Instant::now(),
            )
            .map(|_| "Ready".into())
            .unwrap_or_else(|error| format!("Saving failed: {error}"));
        (final_start, transcript.rows[final_start..].to_vec(), status)
    };
    // Settle provenance in storage before notifying the view, including when
    // its window has closed or the provider was cancelled.
    if let Some((turn, mut info)) = turn_receipt {
        let completed = vorkspace_core::time::now_ms();
        info.duration_ms = info.started_at.map_or(0, |at| completed.saturating_sub(at));
        info.duration_known = true;
        info.completed_at = info.started_at.map(|_| completed);
        if let Err(error) = persistence::save_turn_receipt(&store, summary.id, turn, info) {
            status = format!("{status} · Saving turn metadata failed: {error}");
        }
    }
    if let Err(error) = submission_save {
        status = format!("{status} · Saving local send receipt failed: {error}");
    }
    if let Err(error) = handoff_save {
        status = format!("{status} · Saving handoff failed: {error}");
    }
    // Final status must still arrive after cancellation. Dropping a window closes
    // the receiver, so the worker never keeps a closed viewport alive.
    let _ = tx.send_blocking(Update {
        activity: activity.label(),
        // A stop can make the UI skip intermediate updates. Keep the created
        // checkout in the final receipt so the next Send cannot create it twice.
        worktree_cwd: created_worktree,
        follow_up_delivered: accepted_messages,
        delivered: initial_delivery,
        start,
        rows,
        provider_id,
        usage_limit: if cancelled { None } else { usage_limit },
        handoff,
        status,
        finished: true,
        ..Default::default()
    });
}

/// What a turn's prompt needs to know about the conversation so far.
struct TurnHistory<'a> {
    summary: &'a Session,
    earlier: &'a [vorkspace_core::transcript::Row],
    context: &'a vorkspace_core::context::Usage,
}

/// Prefixes a turn with the Bot or minion context the provider hasn't seen yet.
/// A Bot near its context limit starts a fresh provider session instead.
fn turn_prompt(
    prompt: String,
    settings: &Settings,
    history: TurnHistory<'_>,
    provider_id: &mut Option<String>,
    bot_fingerprint: &mut Option<String>,
    store: &Database,
) -> Result<String, String> {
    if let Some(bot) = &settings.bot {
        let files = vorkspace_core::bot::load_files(&bot.root, &bot.bot)?;
        let added = vorkspace_core::bot::context(&bot.bot, &files, &bot.executable);
        let fingerprint = vorkspace_core::bot::files::content_hash(&added);
        *bot_fingerprint = Some(fingerprint.clone());
        let rotated = provider_id.is_some() && history.context.ratio().is_some_and(|r| r >= 0.8);
        if rotated {
            *provider_id = None;
            locked(store)?.reset_provider_id(history.summary.id)?;
        }
        let receipt = locked(store)?.setting(&format!("bot-context:{}", history.summary.id))?;
        let expected = serde_json::json!({"provider":provider_id,"hash":fingerprint}).to_string();
        let intro = if provider_id.is_none() || receipt.as_deref() != Some(&expected) {
            added
        } else {
            String::new()
        };
        let brief = if provider_id.is_none() && !history.earlier.is_empty() {
            vorkspace_core::bot::rotation_brief(history.earlier)
        } else {
            String::new()
        };
        let prompt = if let Some(handoff) = &settings.handoff {
            vorkspace_core::response::wrap_handoff_prompt(&handoff.from, &handoff.brief, &prompt)
        } else {
            prompt
        };
        Ok(format!("{intro}\n\n{brief}\n\n{prompt}"))
    } else if let Some(minion) = &settings.minion {
        // Its parent's soul and memory, re-sent whenever they change.
        let files = vorkspace_core::bot::load_files(&minion.root, &minion.bot)?;
        let added = vorkspace_core::bot::minion_context(
            &minion.bot,
            &minion.name,
            &history.summary.cwd,
            &files,
            &minion.executable,
        );
        let fingerprint = vorkspace_core::bot::files::content_hash(&added);
        *bot_fingerprint = Some(fingerprint.clone());
        let receipt = locked(store)?.setting(&format!("bot-context:{}", history.summary.id))?;
        let expected = serde_json::json!({"provider":provider_id,"hash":fingerprint}).to_string();
        let intro = if provider_id.is_none() || receipt.as_deref() != Some(&expected) {
            added
        } else {
            String::new()
        };
        let prompt = continuing_prompt(
            settings.handoff.as_ref(),
            provider_id.is_none(),
            history.earlier,
            prompt,
        );
        Ok(format!("{intro}\n\n{prompt}"))
    } else {
        Ok(continuing_prompt(
            settings.handoff.as_ref(),
            provider_id.is_none(),
            history.earlier,
            prompt,
        ))
    }
}

/// checkpointEdits.ts's sessionEditPaths: files named by successful edit tools.
/// A turn's prompt with whatever the provider's thread lacks: a handoff's
/// brief, or a bounded recap of `prior` when a thread starts fresh mid-way,
/// such as after a first turn failed before the provider assigned an ID.
fn continuing_prompt(
    handoff: Option<&vorkspace_core::agent::Handoff>,
    fresh_thread: bool,
    prior: &[Row],
    prompt: String,
) -> String {
    if let Some(handoff) = handoff {
        return vorkspace_core::response::wrap_handoff_prompt(
            &handoff.from,
            &handoff.brief,
            &prompt,
        );
    }
    if !fresh_thread || prior.is_empty() {
        return prompt;
    }
    let mut history = String::new();
    for row in prior
        .iter()
        .rev()
        .filter(|r| matches!(r.role.as_str(), "user" | "assistant"))
    {
        if history.len() + row.text.len() > 32_000 {
            break;
        }
        history = format!("{}: {}\n\n{history}", row.role, row.text);
    }
    format!(
        "Continue this conversation, using the prior messages as context.\n\n{history}\nCurrent user message:\n{prompt}"
    )
}

fn edited_paths(rows: &[Row]) -> Vec<String> {
    let mut paths = Vec::new();
    for row in rows.iter().filter(|r| r.role == "tool") {
        let Ok(tool) = serde_json::from_str::<vorkspace_core::tool::ToolCall>(&row.text) else {
            continue;
        };
        if tool.state != vorkspace_core::tool::ToolState::Completed {
            continue;
        }
        let (action, path) = tool.summary();
        if matches!(
            action.as_str(),
            "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
        ) && !path.is_empty()
            && !paths.contains(&path)
        {
            paths.push(path);
        }
    }
    paths
}

fn shlex_quote(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('\"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`")
    )
}

#[cfg(test)]
mod continuing_prompt_tests {
    use super::*;

    fn row(role: &str, text: &str) -> Row {
        Row {
            role: role.into(),
            text: text.into(),
            first: false,
            sent_at: None,
        }
    }

    #[test]
    fn a_fresh_thread_recaps_the_saved_task_and_stays_bounded() {
        // A first turn failed before the provider assigned a thread.
        let prior = [
            row("user", "Scout the repository layout"),
            row("tool", "ignored"),
            row("assistant", "The provider was unavailable"),
        ];
        let prompt = continuing_prompt(None, true, &prior, "try again".into());
        assert!(prompt.contains("user: Scout the repository layout"));
        assert!(prompt.contains("assistant: The provider was unavailable"));
        assert!(prompt.ends_with("Current user message:\ntry again"));
        assert!(!prompt.contains("ignored"));
        // The newest messages are the ones kept.
        let long = [row("user", &"old ".repeat(9_000)), row("user", "recent")];
        let prompt = continuing_prompt(None, true, &long, "next".into());
        assert!(prompt.contains("recent"));
        assert!(!prompt.contains("old old"));
    }

    #[test]
    fn nothing_is_added_when_the_thread_already_has_it() {
        let prior = [row("user", "Earlier")];
        assert_eq!(
            continuing_prompt(None, false, &prior, "next".into()),
            "next"
        );
        assert_eq!(continuing_prompt(None, true, &[], "first".into()), "first");
    }

    #[test]
    fn a_handoff_takes_the_place_of_the_recap() {
        let handoff = vorkspace_core::agent::Handoff {
            from: "codex".into(),
            brief: "The brief".into(),
        };
        let prior = [row("user", "Earlier")];
        let prompt = continuing_prompt(Some(&handoff), true, &prior, "next".into());
        assert!(prompt.contains("The brief"));
        assert!(!prompt.contains("Earlier"));
    }
}

#[cfg(test)]
mod unsupported_history_tests {
    use super::*;

    #[test]
    fn retired_history_rejects_a_turn_before_resolution_or_durable_mutation() {
        for provider in [
            "cursor", "grok", "opencode", "fx", "hermes", "devin", "unknown",
        ] {
            let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
            let session = store.create("codex", "/history").unwrap();
            store.change_provider(session.id, provider).unwrap();
            store.provider_id(session.id, "original-thread").unwrap();
            let settings = Settings {
                revision: 7,
                model: Some("original-model".into()),
                ..Default::default()
            };
            store.save_agent_settings(session.id, &settings).unwrap();
            let rows = vec![Row {
                role: "assistant".into(),
                text: "Retained historical work".into(),
                first: true,
                sent_at: Some(123),
            }];
            store.write_rows(session.id, 0, &rows).unwrap();
            let summary = store.sessions().unwrap().remove(0);
            let store = Arc::new(Mutex::new(store));
            let (_, controls) = std::sync::mpsc::channel();
            let (tx, rx) = async_channel::bounded(8);
            run_turn_using(
                TurnInput {
                    summary: summary.clone(),
                    prompt: "Continue".into(),
                    settings: settings.clone(),
                },
                controls,
                store.clone(),
                Arc::new(AtomicBool::new(false)),
                tx,
                |_| panic!("a retired provider must never reach CLI resolution"),
            );
            let update = rx.recv_blocking().unwrap();
            assert!(update.finished);
            assert!(update.status.contains("no longer supported"));
            assert!(update.rows.is_empty());
            let store = locked(&store).unwrap();
            let after = store.sessions().unwrap().remove(0);
            assert_eq!(after.provider, provider);
            assert_eq!(after.provider_id, Some("original-thread".into()));
            assert_eq!(
                serde_json::to_value(store.load(session.id).unwrap().rows).unwrap(),
                serde_json::to_value(rows).unwrap()
            );
            assert_eq!(
                serde_json::to_value(store.agent_settings(session.id).unwrap().unwrap()).unwrap(),
                serde_json::to_value(settings).unwrap()
            );
            assert!(store.local_submissions(session.id).unwrap().is_empty());
        }
    }
}
