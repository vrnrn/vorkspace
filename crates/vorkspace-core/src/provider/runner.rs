//! Provider process admission, control, batching, and settlement.
use super::*;
use std::collections::HashSet;

const BATCH_BYTES: usize = 64 * 1024;

/// A tool's output changes in place between publications. Record its first
/// position in the batch, then materialize the latest snapshot once.
#[derive(Default)]
struct Batch {
    events: Vec<Event>,
    updates: Vec<(usize, String)>,
    queued: HashSet<String>,
    bytes: usize,
}

impl Batch {
    fn push(&mut self, event: Event) {
        self.bytes = self.bytes.saturating_add(match &event {
            Event::Text(text) | Event::Reasoning(text) | Event::Status(text) => text.len(),
            Event::Tool(tool) => tool.input.len() + tool.output.len(),
            Event::Approval(approval) => approval.detail.len(),
            Event::Question(question) => question.items.iter().map(|q| q.prompt.len()).sum(),
            _ => 64,
        });
        self.events.push(event);
    }

    fn extend(&mut self, events: impl IntoIterator<Item = Event>) {
        for event in events {
            self.push(event);
        }
    }

    fn tool_output(&mut self, id: String, bytes: usize) {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.queued.insert(id.clone()) {
            self.updates.push((self.events.len(), id));
        }
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty() && self.updates.is_empty()
    }

    fn take(&mut self, protocol: &Protocol) -> Vec<Event> {
        self.bytes = 0;
        self.queued.clear();
        let events = std::mem::take(&mut self.events);
        if self.updates.is_empty() {
            return events;
        }
        let mut result = Vec::with_capacity(events.len() + self.updates.len());
        let mut events = events.into_iter();
        let mut consumed = 0;
        for (at, id) in self.updates.drain(..) {
            result.extend(events.by_ref().take(at - consumed));
            consumed = at;
            if let Some(tool) = protocol.tool_snapshot(&id) {
                result.push(Event::Tool(tool));
            }
        }
        result.extend(events);
        result
    }
}

/// Stream at bounded memory cost. `emit` implements backpressure and must observe
/// cancellation while waiting for the viewport. Errors are returned after reaping.
pub fn run(
    request: Request,
    cancel: Arc<AtomicBool>,
    emit: impl FnMut(Vec<Event>) -> Result<(), String>,
) -> Result<(), String> {
    run_with_controls(request, cancel, None, emit)
}
pub fn run_with_controls(
    request: Request,
    cancel: Arc<AtomicBool>,
    controls: Option<mpsc::Receiver<Control>>,
    emit: impl FnMut(Vec<Event>) -> Result<(), String>,
) -> Result<(), String> {
    run_operation(
        request,
        cancel,
        controls,
        Operation::Turn(DEFAULT_TEXT_TIMEOUT),
        emit,
    )
    .map(|_| ())
}

/// A bounded, separate conversation for titles and other background text work.
/// Never resume the user's conversation or inherit its app capabilities/tools.
pub fn run_text(request: Request, cancel: Arc<AtomicBool>) -> Result<String, String> {
    run_text_with_timeout(request, cancel, DEFAULT_TEXT_TIMEOUT)
}

/// Providers available for background text generation.
pub fn supports_text_only(kind: Kind) -> bool {
    matches!(kind, Kind::Codex | Kind::Claude) || kind.is_pi()
}

pub fn run_text_with_timeout(
    request: Request,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<String, String> {
    run_background_text(request, cancel, Operation::Turn(timeout))
}

/// A background request that verifies Codex restrictions before delivering input.
/// Return one completed final answer only after successful turn completion;
/// streamed narration and reasoning never form the response.
pub fn run_restricted_text(
    request: Request,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<String, String> {
    run_background_text(request, cancel, Operation::RestrictedText(timeout))
}

fn run_background_text(
    mut request: Request,
    cancel: Arc<AtomicBool>,
    operation: Operation,
) -> Result<String, String> {
    if !supports_text_only(request.kind) {
        return Err("This provider cannot run tool-free background text requests.".into());
    }
    request.provider_id = None;
    request.settings = Settings {
        account: request.settings.account.clone(),
        text_only: true,
        access: AccessMode::ReadOnly,
        // Explicit summary effort is independent of chat defaults. Keep the
        // inexpensive title-generation default without overriding summaries.
        effort: request.settings.effort.clone().or_else(|| {
            (request.kind == Kind::Codex && !matches!(operation, Operation::RestrictedText(_)))
                .then(|| "low".into())
        }),
        ..Default::default()
    };
    let mut output = String::new();
    run_operation(request, cancel, None, operation, |events| {
        for event in events {
            match event {
                Event::Text(text) if output.len() + text.len() <= 64 * 1024 => {
                    output.push_str(&text)
                }
                Event::Text(_) => return Err("Background text response exceeded its limit".into()),
                Event::Tool(tool) if tool.is_compaction() => {}
                Event::Tool(_) | Event::Approval(_) | Event::Question(_) => {
                    return Err(
                        "Background text request attempted to use tools or request input".into(),
                    );
                }
                _ => {}
            }
        }
        Ok(())
    })?;
    Ok(output)
}

pub(crate) fn catalog_for(kind: Kind, account: crate::accounts::Profile) -> Result<Value, String> {
    let binary = resolve(kind).ok_or_else(|| {
        format!(
            "{} CLI not installed. Sign in with {}.",
            kind.label(),
            kind.login_hint()
        )
    })?;
    let cwd = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
    run_operation(
        Request {
            kind,
            binary,
            cwd,
            prompt: String::new(),
            provider_id: None,
            model: None,
            settings: Settings {
                account,
                access: AccessMode::ReadOnly,
                ..Default::default()
            },
        },
        Arc::new(AtomicBool::new(false)),
        None,
        Operation::Catalog,
        |_| Ok(()),
    )?
    .ok_or_else(|| format!("{} returned no model catalog", kind.label()))
}

const DEFAULT_TEXT_TIMEOUT: Duration = Duration::from_secs(45);

pub(super) enum Operation {
    Turn(Duration),
    RestrictedText(Duration),
    Catalog,
    Compact,
    Commands,
}

pub(super) fn run_operation(
    mut request: Request,
    cancel: Arc<AtomicBool>,
    controls: Option<mpsc::Receiver<Control>>,
    operation: Operation,
    mut emit: impl FnMut(Vec<Event>) -> Result<(), String>,
) -> Result<Option<Value>, String> {
    let probe = matches!(operation, Operation::Catalog);
    let compact = matches!(operation, Operation::Compact);
    let command_probe = matches!(operation, Operation::Commands);
    let restricted_text = matches!(operation, Operation::RestrictedText(_));
    let restricted_codex = restricted_text && request.kind == Kind::Codex;
    let text_timeout = match operation {
        Operation::Turn(timeout) | Operation::RestrictedText(timeout) => timeout,
        _ => DEFAULT_TEXT_TIMEOUT,
    };
    if !request.kind.is_supported() {
        return Err("This historical provider is no longer supported. Choose a supported provider for a new conversation.".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Stopped".into());
    }
    if !probe && !compact && !command_probe {
        crate::ticket_context::admit_request(
            &request.prompt,
            &request.cwd,
            request.settings.text_only,
            request.settings.ticket_context_data_dir.as_deref(),
            &mut request.settings.ticket_context,
            &cancel,
        )?;
    }
    if !probe && !compact && !command_probe {
        crate::ticket_context::prepare_request(
            &mut request.prompt,
            &request.cwd,
            request.settings.ticket_context_data_dir.as_deref(),
            &mut request.settings.ticket_context,
            &cancel,
        )?;
    }
    let claude_new_session = (request.kind == Kind::Claude
        && request.provider_id.is_none()
        && !request.settings.text_only)
        .then(lifecycle::claude_session_id);
    if std::env::var_os("VORKSPACE_NATIVE_TRACE").is_some() {
        unsafe {
            let mut signals = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut signals);
            eprintln!(
                "provider-launch sigchld_blocked={}",
                libc::sigismember(&signals, libc::SIGCHLD)
            );
        }
    }
    let mut command = Command::new(&request.binary);
    if request.kind == Kind::Claude {
        crate::shell_env::apply_claude_env(&mut command);
    }
    crate::shell_env::apply_gui_env(&mut command);
    command.envs(&request.settings.environment);
    request.settings.account.apply(request.kind, &mut command)?;
    match request.kind {
        Kind::Codex => {
            command.arg("app-server");
            if restricted_codex {
                codex_text::configure(&mut command);
            } else {
                crate::visual_bridge::configure_codex(&mut command, &request.settings.environment);
            }
            if let Some(bot) = &request.settings.bot {
                if let Some(home) = crate::bot::codex_home(&bot.root, &request.settings.account.id)?
                {
                    let sqlite_home = serde_json::to_string(&home).map_err(|e| e.to_string())?;
                    command
                        .env("CODEX_HOME", &home)
                        .env("CODEX_SQLITE_HOME", &home)
                        .args(["-c", &format!("sqlite_home={sqlite_home}")]);
                }
            }
        }
        Kind::Claude => {
            if restricted_text {
                command.args(["--system-prompt", text_output::INSTRUCTIONS]);
            }
            command.args([
                "--verbose",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--include-partial-messages",
                "--permission-prompt-tool",
                "stdio",
            ]);
            let settings = &request.settings;
            let mode = if settings.plan {
                "plan"
            } else {
                settings.access.claude()
            };
            command.args(["--permission-mode", mode]);
            if settings.text_only {
                command.args([
                    "--tools",
                    "",
                    "--setting-sources",
                    "",
                    "--max-turns",
                    "1",
                    "--no-session-persistence",
                    "--strict-mcp-config",
                    "--mcp-config",
                    "{\"mcpServers\":{}}",
                ]);
            } else if settings.access == AccessMode::ReadOnly {
                command.args(["--tools", "Read,Glob,Grep,Bash", "--allowedTools", "Read,Glob,Grep,Bash(git status *),Bash(git diff *),Bash(git log *),Bash(git show *),Bash(pwd),Bash(ls *),Bash(rg *)"]);
            }
            if !settings.text_only {
                if let Some(bot) = &settings.bot {
                    for project in &bot.bot.projects {
                        if std::path::Path::new(project).is_absolute() {
                            command.args(["--add-dir", project]);
                        }
                    }
                }
                if let Some(server) = crate::visual_bridge::mcp_server(&settings.environment) {
                    command.args([
                        "--mcp-config",
                        &json!({"mcpServers":{"vorkspace_visual":server}}).to_string(),
                    ]);
                }
            }
            if mode == "bypassPermissions" {
                command.arg("--allow-dangerously-skip-permissions");
            }
            let mut config = json!({});
            if settings.disable_claude_hooks || settings.text_only {
                config["disableAllHooks"] = true.into();
            }
            if settings.fast {
                config["fastMode"] = true.into();
            }
            if settings.thinking == Some(true) {
                config["alwaysThinkingEnabled"] = true.into();
            }
            if let Some(effort) = settings.effort.as_deref().filter(|e| *e != "ultrathink") {
                let effort = if effort == "ultracode" {
                    config["ultracode"] = true.into();
                    "xhigh"
                } else if effort == "xhigh"
                    && request.model.as_deref().is_some_and(|m| {
                        matches!(
                            m,
                            "claude-opus-4-6"
                                | "claude-sonnet-4-6"
                                | "claude-opus-4-5"
                                | "claude-haiku-4-5"
                                | "claude-opus-4-1"
                                | "claude-opus-4-0"
                                | "claude-sonnet-4-0"
                                | "claude-sonnet-4-5"
                        )
                    })
                {
                    "max"
                } else if effort == "max" && request.model.as_deref() == Some("claude-sonnet-4-6") {
                    "high"
                } else {
                    effort
                };
                command.args(["--effort", effort]);
            }
            command.args(["--settings", &config.to_string()]);
            if !settings.text_only {
                command.arg("--setting-sources=user,project,local");
            }
            if let Some(id) = &request.provider_id {
                command.args(["--resume", id]);
            } else if let Some(id) = &claude_new_session {
                command.args(["--session-id", id]);
            }
            if let Some(model) = &request.model {
                let model = if settings
                    .model_settings
                    .get("context")
                    .is_some_and(|value| value == "1m")
                    && !model.ends_with("[1m]")
                    && !model.starts_with("copilot/")
                {
                    format!("{model}[1m]")
                } else {
                    model.clone()
                };
                command.args(["--model", &model]);
            }
        }
        kind if kind.is_pi() => {
            if restricted_text {
                command.args(["--system-prompt", text_output::INSTRUCTIONS]);
            }
            pi::configure(&mut command, &request, probe || command_probe);
        }
        _ => acp::configure(&request)?,
    }
    command
        .current_dir(if request.kind == Kind::Antigravity {
            request.binary.parent().unwrap_or(&request.cwd)
        } else {
            &request.cwd
        })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process::isolate(&mut command);
    if cancel.load(Ordering::Relaxed) {
        return Err("Stopped".into());
    }
    let eligible = !probe && !command_probe && !request.settings.text_only;
    let cached = eligible
        .then(|| parked::take(&parked::key(&request)))
        .flatten();
    let (mut wire, mut protocol, warm) = if let Some(cached) = cached {
        let mut protocol = Protocol::new(request);
        cached.continuation.restore(&mut protocol);
        (cached.wire, protocol, true)
    } else {
        if eligible {
            // A new process supersedes older warm launch configurations and
            // any cancelled process still draining for this provider binding.
            parked::discard_session(&request);
        }
        (
            parked::Wire::spawn(&mut command, &request)?,
            Protocol::new(request),
            false,
        )
    };
    let rx = &wire.rx;
    let tail = &wire.tail;
    let mut stdin = &mut wire.stdin;
    let mut pending = Batch::default();
    if let Some(id) = claude_new_session {
        protocol.request.provider_id = Some(id.clone());
        pending.push(Event::SessionId(id));
    }
    let mut codex_setup = restricted_codex.then_some(codex_text::Setup::Initialize);
    if restricted_text {
        protocol.restricted_output = Some(text_output::Output::new(protocol.request.kind));
    }
    let mut stop_confirmed = false;
    let result = (|| {
        protocol.probe = probe;
        protocol.compact = compact;
        protocol.command_probe = command_probe;
        if warm {
            if protocol.request.kind.is_acp() {
                protocol.acp_resume_warm(&mut stdin)?;
            } else if protocol.request.kind.is_pi() {
                protocol.pi_start(&mut stdin)?;
            } else if protocol.request.kind == Kind::Codex {
                protocol.initialized = true;
                protocol.decode_frame(json!({"id":1,"result":{}}), &mut stdin)?;
            } else {
                protocol.follow_up_ready = true;
                protocol.decode_frame(
                    json!({"type":"control_response","response":{"request_id":"native-init"}}),
                    &mut stdin,
                )?;
            }
        } else if protocol.request.kind.is_acp() {
            protocol.acp_start(&mut stdin)?;
        } else if protocol.request.kind.is_pi() {
            protocol.pi_start(&mut stdin)?;
        } else if protocol.request.kind == Kind::Codex {
            write_json(
                &mut stdin,
                json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"vorkspace_native","title":"vorkspace Native","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}}),
            )?;
        } else {
            write_json(
                &mut stdin,
                json!({"type":"control_request","request_id":"native-init","request":{"subtype":"initialize"}}),
            )?;
        }
        let visual_bridge =
            crate::visual_bridge::configured(&protocol.request.settings.environment);
        let mut last_emit = Instant::now();
        let started = Instant::now();
        let mut ready_emitted = false;
        let mut interrupted = false;
        loop {
            if cancel.load(Ordering::Relaxed) && !interrupted {
                interrupted = true;
                if !protocol.interrupt(&mut stdin)? {
                    return Err("Stopped".into());
                }
                if compact || protocol.request.kind == Kind::Claude {
                    return Err("Stopped".into());
                }
            }
            if !interrupted {
                if let Some(controls) = &controls {
                    while let Ok(control) = controls.try_recv() {
                        if let Decision::Steer(message) = control.decision {
                            let id = message.id.clone();
                            match protocol.steer(message, &mut stdin, &cancel, |message| {
                                emit(vec![Event::FollowUpDispatch(message.clone())])
                            }) {
                                Ok(events) => pending.extend(events),
                                Err(error) => pending.push(Event::FollowUpFailed { id, error }),
                            }
                        } else {
                            protocol.respond(control, &mut stdin)?;
                        }
                    }
                }
                pending.extend(protocol.expire_steering());
                pending.extend(protocol.lifecycle_tick(started, &mut stdin)?);
                if protocol.ready() && !ready_emitted {
                    ready_emitted = true;
                    pending.push(Event::Ready);
                }
            }
            if interrupted && protocol.request.kind.is_pi() {
                let _ = protocol.pi_tick(&mut stdin);
            }
            if interrupted && protocol.done {
                stop_confirmed = true;
                return Err("Stopped".into());
            }
            if protocol.request.settings.text_only && started.elapsed() >= text_timeout {
                return Err("Background text request timed out".into());
            }
            match rx.recv_timeout(TICK) {
                Ok(Ok(line)) => {
                    let value: Value = match serde_json::from_slice(&line) {
                        Ok(value) => value,
                        // Some CLIs print startup banners before their NDJSON stream.
                        Err(_)
                            if !line
                                .iter()
                                .copied()
                                .find(|b| !b.is_ascii_whitespace())
                                .is_some_and(|b| b == b'{') =>
                        {
                            continue;
                        }
                        Err(e) => return Err(format!("Invalid provider JSON: {e}")),
                    };
                    if std::env::var_os("VORKSPACE_NATIVE_TRACE").is_some() {
                        let method = value["method"].as_str().unwrap_or("");
                        let item = &value["params"]["item"];
                        if !method.is_empty()
                            && !method.ends_with("Delta")
                            && !method.ends_with("/delta")
                        {
                            eprintln!(
                                "provider-frame method={method} type={} status={} id={} output_bytes={} keys={:?} legacy={}",
                                item["type"],
                                item["status"],
                                item["id"],
                                item["aggregatedOutput"].as_str().map_or(0, str::len),
                                value["params"]
                                    .as_object()
                                    .map(|o| o.keys().collect::<Vec<_>>()),
                                value["params"]["msg"]["type"]
                            );
                        }
                    }
                    if interrupted {
                        let reply_id = value["id"].as_str();
                        let turn_id = value["params"]["turnId"]
                            .as_str()
                            .or_else(|| value["params"]["turn"]["id"].as_str());
                        let terminal = matches!(
                            value["method"].as_str(),
                            Some("turn/completed" | "turn/aborted")
                        ) && (turn_id.is_some()
                            && turn_id == protocol.active_turn.as_deref()
                            || turn_id.is_none() && protocol.previous_turn.is_none());
                        let codex_ack = protocol.request.kind == Kind::Codex
                            && (reply_id.is_some() && reply_id == protocol.interrupt_id.as_deref()
                                || terminal);
                        let claude_end =
                            protocol.request.kind == Kind::Claude && value["type"] == "result";
                        if codex_ack || claude_end {
                            stop_confirmed = value.get("error").is_none_or(Value::is_null);
                            if terminal || claude_end {
                                protocol.done = true;
                            }
                            return Err("Stopped".into());
                        }
                    }
                    let value = if let Some(setup) = &mut codex_setup {
                        let Some(value) = setup.receive(value, &protocol.request, &mut stdin)?
                        else {
                            continue;
                        };
                        if value["id"] == 2 && cancel.load(Ordering::Acquire) {
                            return Err("Stopped".into());
                        }
                        value
                    } else {
                        value
                    };
                    let events = match protocol.decode_frame(value, &mut stdin) {
                        Ok(DecodedFrame::Events(events)) => events,
                        Ok(DecodedFrame::ToolOutput { id, bytes }) => {
                            if !interrupted {
                                pending.tool_output(id, bytes);
                            }
                            Vec::new()
                        }
                        Err(_) if interrupted => return Err("Stopped".into()),
                        Err(error) => return Err(error),
                    };
                    for event in events {
                        if controls.is_none() || interrupted {
                            if let Event::Approval(approval) = &event {
                                protocol.respond(
                                    Control {
                                        id: approval.id.clone(),
                                        decision: Decision::Deny,
                                    },
                                    &mut stdin,
                                )?;
                            }
                            if let Event::Question(question) = &event {
                                protocol.respond(
                                    Control {
                                        id: question.id.clone(),
                                        decision: Decision::Deny,
                                    },
                                    &mut stdin,
                                )?;
                            }
                        }
                        if interrupted {
                            continue;
                        }
                        if let Event::SessionId(id) = &event {
                            protocol.request.provider_id = Some(id.clone());
                        }
                        pending.push(event);
                    }
                }
                Ok(Err(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if interrupted {
                        return Err("Stopped".into());
                    }
                    return Err(format!(
                        "Provider exited before completing the turn. {}",
                        String::from_utf8_lossy(&tail.lock().unwrap())
                    ));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            // A visual can arrive while its provider is blocked awaiting the
            // tool reply. Give it the same cadence as ordinary publication.
            let tick = last_emit.elapsed() >= TICK;
            if (!pending.is_empty() && (tick || pending.bytes >= BATCH_BYTES || protocol.done))
                || (visual_bridge && tick)
            {
                emit(pending.take(&protocol))?;
                last_emit = Instant::now();
            }
            if protocol.done {
                if interrupted {
                    stop_confirmed = true;
                    return Err("Stopped".into());
                }
                if let Some(output) = protocol.restricted_output.take() {
                    emit(vec![Event::Text(output.finish()?)])?;
                }
                return Ok(protocol.catalog_result.take());
            }
            if command_probe && started.elapsed() > Duration::from_secs(45) {
                return Err("Command discovery timed out".into());
            }
            let discovery_timeout = match protocol.request.kind {
                Kind::Pi | Kind::Omp => Some(45),
                Kind::Antigravity => None,
                _ => Some(60),
            };
            if probe
                && discovery_timeout
                    .is_some_and(|seconds| started.elapsed() > Duration::from_secs(seconds))
            {
                return Err("Model discovery timed out".into());
            }
        }
    })();
    // Cancellation can arrive while publishing an event batch. Its callback
    // then returns Stopped before the next receive-loop iteration can interrupt.
    if cancel.load(Ordering::Relaxed) && protocol.interrupt_id.is_none() {
        let _ = protocol.interrupt(&mut wire.stdin);
    }
    if cancel.load(Ordering::Relaxed) && protocol.done {
        stop_confirmed = true;
    }
    if result.is_err() && !cancel.load(Ordering::Relaxed) {
        if let Some(limit) = protocol.claude_usage_limit.take() {
            pending.push(Event::UsageLimited(limit));
        }
    }
    if !pending.is_empty() {
        let _ = emit(pending.take(&protocol));
    }
    // Compaction releases its exclusive local claim when this worker returns.
    // Reap an unfinished cancelled child here instead of parking its drain.
    if compact && cancel.load(Ordering::Relaxed) && !protocol.done {
        return result;
    }
    if eligible
        && cancel.load(Ordering::Relaxed)
        && !protocol.done
        && protocol.request.kind != Kind::Antigravity
        && protocol.interrupt_id.is_some()
    {
        parked::settle_cancelled(wire, protocol);
    } else if eligible
        && ((result.is_ok() && !cancel.load(Ordering::Relaxed))
            || (stop_confirmed && protocol.request.kind != Kind::Antigravity))
    {
        parked::put(wire, protocol);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_output_in_a_batch_publishes_once_at_its_first_position() {
        let mut protocol = Protocol::new(Request {
            kind: Kind::Codex,
            binary: PathBuf::new(),
            cwd: PathBuf::new(),
            prompt: String::new(),
            provider_id: None,
            model: None,
            settings: Settings::default(),
        });
        let mut call = ToolCall::new("running", "commandExecution");
        call.append_output("latest output");
        protocol.tools.insert(call.id.clone(), call.clone());
        let mut batch = Batch::default();
        batch.push(Event::Text("before".into()));
        batch.tool_output("running".into(), 6);
        batch.push(Event::Text("after".into()));
        batch.tool_output("running".into(), 7);
        assert_eq!(
            batch.take(&protocol),
            [
                Event::Text("before".into()),
                Event::Tool(call),
                Event::Text("after".into()),
            ]
        );
        assert!(batch.is_empty());
        assert_eq!(batch.bytes, 0);
    }
}
