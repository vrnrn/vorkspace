//! Explicit provider rollback. This never starts a prompt or edits local history.
//! The methods and message shapes mirror the checked-in Codex and Pi adapters.
use super::*;

#[derive(Debug)]
pub struct RewindError {
    pub message: String,
    /// A destructive request may have reached the provider without its reply.
    pub provider_may_have_changed: bool,
}
impl From<String> for RewindError {
    fn from(message: String) -> Self {
        Self {
            message,
            provider_may_have_changed: false,
        }
    }
}
impl From<&str> for RewindError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl std::fmt::Display for RewindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RewindError {}
impl RewindError {
    fn uncertain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            provider_may_have_changed: true,
        }
    }
}

pub use crate::kind::supports_rewind;

/// Runs on a worker, after the caller has reserved its conversation. Returns the
/// provider binding to persist with the confirmed local truncation. Pi forks to
/// a new session; Codex keeps the current thread. No prompt is sent here.
pub fn rewind_last_turn(
    request: Request,
    before_turn: Option<&str>,
) -> Result<String, RewindError> {
    if !supports_rewind(request.kind) || request.settings.text_only {
        return Err("This provider does not support editing the last turn".into());
    }
    let session = request
        .provider_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .ok_or("The conversation has no saved provider session to rewind")?;
    // Remove the previous warm process before reverting its underlying session.
    // It must not later resume with a pre-revert in-memory context.
    parked::discard_session(&request);
    let mut command = Command::new(&request.binary);
    crate::shell_env::apply_gui_env(&mut command);
    command.envs(&request.settings.environment);
    request.settings.account.apply(request.kind, &mut command)?;
    if request.kind == Kind::Codex {
        command.arg("app-server");
    } else {
        pi::configure(&mut command, &request, false);
    }
    command
        .current_dir(&request.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process::isolate(&mut command);
    let mut wire = parked::Wire::spawn(&mut command, &request)?;
    let mut rpc = Rpc {
        wire: &mut wire,
        serial: 100,
        pi: request.kind.is_pi(),
    };
    if rpc.pi {
        let state = rpc.call("get_state", json!({}), Effect::Read)?;
        if pi_binding(&state).as_deref() != Some(session) {
            return Err("Pi resumed a different session; history was preserved".into());
        }
        if state["isStreaming"] == true || state["isCompacting"] == true {
            return Err("Stop the current turn before editing the last message".into());
        }
        let messages = rpc.call("get_fork_messages", json!({}), Effect::Read)?;
        let entry = last_fork_entry(&messages)?;
        let fork = rpc.call("fork", json!({"entryId":entry}), Effect::Change)?;
        if fork["cancelled"] == true {
            return Err("Edit cancelled".into());
        }
        let state = rpc.call("get_state", json!({}), Effect::AfterChange)?;
        return pi_binding(&state)
            .filter(|binding| binding != session)
            .ok_or_else(|| RewindError::uncertain("Pi did not expose a new forked session"));
    }
    rpc.call("initialize", json!({"clientInfo":{"name":"vorkspace_native","title":"vorkspace Native","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}), Effect::Read)?;
    write_json(&mut rpc.wire.stdin, json!({"method":"initialized"}))?;
    validate_codex_resume(
        &rpc.call(
            "thread/resume",
            json!({"threadId":session,"cwd":request.cwd}),
            Effect::Read,
        )?,
        session,
    )?;
    let before_turn = if let Some(id) = before_turn.filter(|id| !id.trim().is_empty()) {
        id.to_owned()
    } else {
        let page = rpc.call(
            "thread/turns/list",
            json!({"threadId":session,"limit":100,"sortDirection":"desc","itemsView":"summary"}),
            Effect::Read,
        )?;
        page["data"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|turn| {
                turn["items"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["type"] == "userMessage"))
            })
            .and_then(|turn| turn["id"].as_str())
            .ok_or("Codex did not expose a user turn id to edit")?
            .to_owned()
    };
    rpc.call(
        "thread/revert",
        json!({"threadId":session,"beforeTurnId":before_turn}),
        Effect::Change,
    )?;
    Ok(session.into())
}

fn validate_codex_resume(resumed: &Value, session: &str) -> Result<(), RewindError> {
    if resumed["thread"]["id"].as_str() != Some(session) {
        return Err("Codex resumed a different thread; history was preserved".into());
    }
    if resumed["thread"]["turns"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|turn| matches!(turn["status"].as_str(), Some("inProgress" | "running")))
    {
        return Err("Stop the current turn before editing the last message".into());
    }
    Ok(())
}

fn last_fork_entry(data: &Value) -> Result<String, String> {
    data["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find_map(|item| item["entryId"].as_str().filter(|id| !id.trim().is_empty()))
        .map(str::to_owned)
        .ok_or_else(|| "No user message to edit".into())
}
fn pi_binding(state: &Value) -> Option<String> {
    let id = state["sessionId"]
        .as_str()
        .or_else(|| state["session_id"].as_str())?
        .trim();
    (!id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| id.to_owned())
}
struct Rpc<'a> {
    wire: &'a mut parked::Wire,
    serial: u64,
    pi: bool,
}

#[derive(Clone, Copy)]
enum Effect {
    Read,
    Change,
    AfterChange,
}

impl Effect {
    fn transport_error(self, message: String) -> RewindError {
        match self {
            Self::Read => message.into(),
            Self::Change | Self::AfterChange => RewindError::uncertain(message),
        }
    }

    fn rejected(self, message: String) -> RewindError {
        match self {
            Self::Read | Self::Change => message.into(),
            Self::AfterChange => RewindError::uncertain(message),
        }
    }
}

impl Rpc<'_> {
    fn call(&mut self, method: &str, params: Value, effect: Effect) -> Result<Value, RewindError> {
        let timeout = Duration::from_secs(match method {
            "get_state" | "thread/resume" | "thread/revert" => 45,
            _ => 15,
        });
        self.call_with_timeout(method, params, effect, timeout)
    }

    fn call_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        effect: Effect,
        timeout: Duration,
    ) -> Result<Value, RewindError> {
        self.serial += 1;
        let id = if self.pi {
            json!(format!("native-edit-{}", self.serial))
        } else {
            json!(self.serial)
        };
        let message = if self.pi {
            let mut message = params;
            message["id"] = id.clone();
            message["type"] = method.into();
            message
        } else {
            json!({"id":id,"method":method,"params":params})
        };
        write_json(&mut self.wire.stdin, message).map_err(|error| effect.transport_error(error))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(effect.transport_error(format!(
                    "Provider did not confirm {method}; local history was preserved"
                )));
            }
            match self.wire.rx.recv_timeout(remaining) {
                Ok(Ok(line)) => {
                    let Ok(mut value) = serde_json::from_slice::<Value>(&line) else {
                        continue;
                    };
                    if !self.pi
                        && value["method"] == "currentTime/read"
                        && value.get("id").is_some()
                    {
                        write_json(
                            &mut self.wire.stdin,
                            json!({"id":value["id"],"result":{"currentTimeAt":crate::time::now_ms()/1000}}),
                        ).map_err(|error| effect.transport_error(error))?;
                        continue;
                    }
                    if value["id"] != id {
                        continue;
                    }
                    if let Some(error) = value
                        .get("error")
                        .filter(|error| !self.pi && !error.is_null())
                    {
                        return Err(effect.rejected(codex_error(error, "Provider rejected rewind")));
                    }
                    if self.pi {
                        if value["type"] != "response" {
                            continue;
                        }
                        match value["success"].as_bool() {
                            Some(true) => {
                                return Ok(value
                                    .get_mut("data")
                                    .map(Value::take)
                                    .unwrap_or_default())
                            }
                            Some(false) => {
                                return Err(effect.rejected(
                                    value["error"]
                                        .as_str()
                                        .unwrap_or("Provider rejected rewind")
                                        .to_owned(),
                                ))
                            }
                            None => {
                                return Err(effect.transport_error(format!(
                                    "Provider returned an invalid confirmation for {method}"
                                )))
                            }
                        }
                    }
                    if let Some(result) = value.get_mut("result") {
                        return Ok(result.take());
                    }
                }
                Ok(Err(error)) => return Err(effect.transport_error(error)),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(effect.transport_error(format!(
                        "Provider exited before confirming {method}; local history was preserved"
                    )));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn request(kind: Kind, binary: PathBuf, cwd: &Path) -> Request {
        Request {
            kind,
            binary,
            cwd: cwd.into(),
            prompt: String::new(),
            provider_id: Some("original".into()),
            model: None,
            settings: Default::default(),
        }
    }

    #[test]
    fn lost_destructive_confirmation_requires_recovery_but_explicit_rejection_does_not() {
        assert!(
            Effect::Change
                .transport_error("Lost reply".into())
                .provider_may_have_changed
        );
        assert!(
            !Effect::Read
                .transport_error("Resume failed".into())
                .provider_may_have_changed
        );
        assert!(
            !Effect::Change
                .rejected("Provider rejected rewind".into())
                .provider_may_have_changed
        );
        assert!(
            Effect::AfterChange
                .rejected("Provider rejected get_state".into())
                .provider_may_have_changed
        );
    }
    #[test]
    fn only_native_transports_with_an_explicit_rewind_protocol_advertise_support() {
        for kind in Kind::ALL {
            assert_eq!(
                supports_rewind(kind),
                matches!(kind, Kind::Codex | Kind::Pi | Kind::Omp)
            );
        }
    }
    #[test]
    fn pi_fork_uses_the_last_valid_user_entry_and_rejects_missing_entries() {
        assert_eq!(
            last_fork_entry(&json!({"messages":[{"entryId":"first"},{"entryId":"last"},{}]}))
                .unwrap(),
            "last"
        );
        assert!(last_fork_entry(&json!({"messages":[]})).is_err());
        assert!(last_fork_entry(&json!([])).is_err());
    }
    #[test]
    fn pi_binding_never_persists_a_session_file_as_an_id() {
        assert_eq!(
            pi_binding(&json!({"sessionId":"fork-123"})),
            Some("fork-123".into())
        );
        assert_eq!(pi_binding(&json!({"sessionId":"/tmp/session.jsonl"})), None);
        assert_eq!(pi_binding(&json!({"sessionId":" "})), None);
    }

    #[test]
    fn a_silent_codex_revert_times_out_with_uncertain_provider_state() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "IFS= read -r frame; exec /bin/sleep 60"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process::isolate(&mut command);
        let request = request(Kind::Codex, "/bin/sh".into(), Path::new("/tmp"));
        let mut wire = parked::Wire::spawn(&mut command, &request).unwrap();
        let mut rpc = Rpc {
            wire: &mut wire,
            serial: 100,
            pi: false,
        };
        let error = rpc
            .call_with_timeout(
                "thread/revert",
                json!({"threadId":"original","beforeTurnId":"last-turn"}),
                Effect::Change,
                Duration::from_millis(25),
            )
            .unwrap_err();
        assert!(error.message.contains("did not confirm thread/revert"));
        assert!(error.provider_may_have_changed);
    }

    #[test]
    fn pi_requires_a_confirmed_new_binding_after_fork() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("pi-fixture");
        std::fs::write(
            &binary,
            r#"#!/usr/bin/env python3
import json, os, sys
forked = False
for line in sys.stdin:
    request = json.loads(line)
    method = request['type']
    if method == 'get_state':
        if forked and os.environ['FORK_OUTCOME'] == 'disconnect':
            break
        data = {'sessionId': 'forked' if forked and os.environ['FORK_OUTCOME'] == 'new' else 'original'}
    elif method == 'get_fork_messages':
        data = {'messages': [{'entryId': 'last-user'}]}
    elif method == 'fork':
        assert request['entryId'] == 'last-user'
        forked = True
        data = {'cancelled': False}
    else:
        raise ValueError(method)
    print(json.dumps({'type': 'response', 'id': request['id'], 'success': True, 'data': data}), flush=True)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        for outcome in ["new", "unchanged", "disconnect"] {
            let mut request = request(Kind::Pi, binary.clone(), dir.path());
            request
                .settings
                .environment
                .insert("FORK_OUTCOME".into(), outcome.into());
            let result = rewind_last_turn(request, None);
            if outcome == "new" {
                assert_eq!(result.unwrap(), "forked");
            } else {
                assert!(result.unwrap_err().provider_may_have_changed, "{outcome}");
            }
        }
    }
}
