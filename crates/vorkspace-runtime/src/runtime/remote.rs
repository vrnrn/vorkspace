//! The host owns agent execution. Desktop disconnection never cancels it.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
};
use vorkspace_core::{
    agent::{Approval, Decision, Question, QuestionItem, QuestionOption, RemoteAction},
    remote::protocol::SessionObservation,
    store::{RemoteHistory, RemoteSession, RemoteUpload},
};
fn require_ticket_context(descriptor: &Value) -> Result<(), String> {
    if descriptor["capabilities"]
        .as_array()
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability == "sessions.preparedText")
        })
    {
        Ok(())
    } else {
        Err("This remote host cannot accept ticket context yet. Update the host before sending this message; the ticket reference was not sent.".into())
    }
}

fn update(status: impl Into<String>, outcome: Option<TurnOutcome>) -> Update {
    Update {
        status: status.into(),
        transition: outcome.map_or(TurnTransition::Progress, TurnTransition::settled),
        ..Default::default()
    }
}
fn command(kind: &str, session_id: Option<&str>) -> Value {
    let mut value = json!({"type":kind,"commandId":vorkspace_core::bot::new_id()});
    if let Some(id) = session_id {
        value["sessionId"] = json!(id);
    }
    value
}
pub fn host_model(provider: &str, model: Option<&str>) -> Result<String, String> {
    let model = model
        .filter(|value| !value.trim().is_empty())
        .ok_or("Choose a model on the remote machine before sending")?;
    Ok(if model.starts_with(&format!("{provider}:")) {
        model.into()
    } else {
        format!("{provider}:{model}")
    })
}
fn model_settings(provider: &str, settings: &Settings) -> HashMap<String, String> {
    vorkspace_core::remote::protocol::wire_model_settings(provider, settings)
}
/// Serializes command preparation with idle setting updates for this desktop
/// session, while observation itself never holds an operation lock.
fn operation_lock(store: &Database, id: i64) -> Arc<Mutex<()>> {
    type Locks = Mutex<HashMap<(usize, i64), std::sync::Weak<Mutex<()>>>>;
    static LOCKS: std::sync::OnceLock<Locks> = std::sync::OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    let key = (Arc::as_ptr(store) as usize, id);
    if let Some(lock) = locks.get(&key).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}
fn configure_command(
    provider: &str,
    settings: &Settings,
    session_id: &str,
) -> Result<Value, String> {
    let mut value = command("configure", Some(session_id));
    value["model"] = json!(host_model(provider, settings.model.as_deref())?);
    value["modelSettings"] = json!(model_settings(provider, settings));
    value["runtimeMode"] = json!(settings.access.id());
    Ok(value)
}
/// Called off-thread after saving an idle remote conversation's settings.
/// Read the latest settings after acquiring the operation lock so an older
/// background callback cannot overwrite a more recent model/access choice.
pub fn configure_saved(store: Database, id: i64) -> Result<(), String> {
    let lock = operation_lock(&store, id);
    let _guard = lock.lock().map_err(|_| "Remote session is unavailable")?;
    let (mut binding, settings) = {
        let store = locked(&store)?;
        let Some(binding) = store.remote_session(id)? else {
            return Ok(());
        };
        if binding.session_id.is_none() || !binding.outbox.is_empty() {
            return Ok(());
        }
        (binding, store.agent_preferences(id)?.unwrap_or_default())
    };
    let client = vorkspace_core::remote::default_connections()?;
    let machine = binding.machine(&client)?;
    let snapshot = client.load_session(&machine.id, binding.session_id.as_deref().unwrap())?;
    if snapshot["status"] == "running" {
        return Ok(());
    }
    let configure = configure_command(
        &binding.summary.provider,
        &settings,
        binding.session_id.as_deref().unwrap(),
    )?;
    if configure["model"] == snapshot["session"]["model"]
        && configure["modelSettings"] == snapshot["session"]["modelSettings"]
        && configure["runtimeMode"] == snapshot["session"]["runtimeMode"]
    {
        return Ok(());
    }
    let mut next = binding.clone();
    next.outbox.push(configure);
    commit(&store, &mut binding, next)?;
    dispatch_outbox(&client, &machine.id, &store, &mut binding).map(|_| ())
}

#[cfg(test)]
fn journal(store: &Database, binding: &RemoteSession) -> Result<(), String> {
    locked(store)?.save_remote_session(binding)
}
/// Commit durable state before changing the worker's view of pending commands.
fn commit(
    store: &Database,
    binding: &mut RemoteSession,
    next: RemoteSession,
) -> Result<(), String> {
    *binding = locked(store)?.commit_remote_journal(binding, next)?;
    Ok(())
}
/// The admitted turn retains its identity until the desktop detaches.
#[derive(Debug)]
struct ObservedRun {
    run_id: Option<String>,
    legacy_prompt: Option<String>,
}

impl ObservedRun {
    fn from_receipt(receipt: &Value, command_id: &str) -> Self {
        let run_id = run_id(receipt).map(str::to_owned);
        let legacy_prompt = run_id.is_none().then(|| command_id.to_owned());
        Self {
            run_id,
            legacy_prompt,
        }
    }

    fn accepts(&mut self, snapshot: &Value) -> bool {
        let observed = run_id(snapshot);
        if let Some(expected) = self.run_id.as_deref() {
            // The v1 Host keeps its last run ID at settlement. Older hosts
            // can omit it once idle, when there is no live run to control.
            return observed.map_or(snapshot["status"] != "running", |id| id == expected);
        }
        if snapshot["status"] == "running"
            && let Some(expected) = self.legacy_prompt.as_deref()
        {
            let prompt = snapshot["session"]["blocks"]
                .as_array()
                .and_then(|blocks| {
                    blocks.iter().rev().find(|block| {
                        block["role"] == "user"
                            && block["draft"] != true
                            && block["internal"] != true
                    })
                })
                .and_then(|block| block["id"].as_str());
            if prompt != Some(expected) {
                return false;
            }
        }
        self.run_id = observed.map(str::to_owned);
        true
    }

    fn id(&self) -> Result<&str, String> {
        self.run_id
            .as_deref()
            .ok_or_else(|| "Running host session has no run identity".into())
    }
}

fn run_id(value: &Value) -> Option<&str> {
    value["runId"].as_str().filter(|id| !id.is_empty())
}

/// Commands are written before I/O. A lost response leaves the original ID for
/// Retry or restart recovery. A receipt binds the observation to the accepted run.
fn dispatch_outbox(
    client: &vorkspace_core::remote::Connections,
    machine: &str,
    store: &Database,
    binding: &mut RemoteSession,
) -> Result<Option<ObservedRun>, String> {
    let mut run = None;
    while !binding.outbox.is_empty() {
        if let Some(started) = dispatch_pending(client, machine, store, binding, 0)? {
            run = Some(started);
        }
    }
    Ok(run)
}

fn dispatch_pending(
    client: &vorkspace_core::remote::Connections,
    machine: &str,
    store: &Database,
    binding: &mut RemoteSession,
    index: usize,
) -> Result<Option<ObservedRun>, String> {
    let mut next = binding.outbox[index].clone();
    if next["type"] != "create" && next["sessionId"].is_null() {
        next["sessionId"] = json!(
            binding
                .session_id
                .as_deref()
                .ok_or("Remote session creation has not been confirmed")?
        );
        let mut durable = binding.clone();
        durable.outbox[index] = next.clone();
        commit(store, binding, durable)?;
    }
    let command_id = next["commandId"]
        .as_str()
        .ok_or("Remote command identity is missing")?;
    if binding
        .uploads
        .iter()
        .any(|upload| upload.command_id == command_id)
    {
        return Err("Remote attachments have not finished uploading".into());
    }
    let receipt = match client.request(machine, "commands.dispatch", next.clone()) {
        Ok(receipt) => receipt,
        Err(error) => {
            if error.starts_with("Host rejected request:") {
                let mut durable = binding.clone();
                if matches!(next["type"].as_str(), Some("cancel" | "approve" | "answer")) {
                    // A stale control cannot invalidate an unrelated send.
                    durable.outbox.remove(index);
                } else {
                    durable.rejected = Some(
                        json!({"error":error,"commands":binding.outbox,"uploads":binding.uploads}),
                    );
                    durable.outbox.clear();
                    durable.uploads.clear();
                }
                commit(store, binding, durable)?;
            }
            return Err(error);
        }
    };
    let id = receipt["sessionId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or("Host command receipt has no session identity")?;
    if receipt["commandId"].as_str() != Some(command_id) {
        return Err("Host returned a receipt for another command".into());
    }
    if next["type"] != "create" && binding.session_id.as_deref() != Some(id) {
        return Err("Host returned a receipt for another session".into());
    }
    if receipt["revision"].as_u64().is_none() {
        return Err("Host command receipt has no revision".into());
    }
    let mut durable = binding.clone();
    if next["type"] == "create" {
        durable.session_id = Some(id.into());
    }
    durable.outbox.remove(index);
    commit(store, binding, durable)?;
    Ok(matches!(next["type"].as_str(), Some("send" | "compact"))
        .then(|| ObservedRun::from_receipt(&receipt, command_id)))
}

/// An attached run can be controlled while a different command awaits Retry.
fn dispatch_control(
    client: &vorkspace_core::remote::Connections,
    machine: &str,
    store: &Database,
    binding: &mut RemoteSession,
    command: Value,
) -> Result<(), String> {
    let lock = operation_lock(store, binding.summary.id);
    let _guard = lock.lock().map_err(|_| "Remote session is unavailable")?;
    let mut current = locked(store)?
        .remote_session(binding.summary.id)?
        .filter(|current| current.same_binding(binding))
        .ok_or("Remote conversation changed before the request could be sent")?;
    let index = current.outbox.len();
    let mut next = current.clone();
    next.outbox.push(command);
    commit(store, &mut current, next)?;
    let result = dispatch_pending(client, machine, store, &mut current, index);
    *binding = current;
    result.map(|_| ())
}
const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
const ATTACHMENT_CHUNK_BYTES: usize = 512 * 1024;

fn attachment_type(path: &std::path::Path) -> (&'static str, &'static str) {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => ("image/png", "image"),
        "jpg" | "jpeg" => ("image/jpeg", "image"),
        "webp" => ("image/webp", "image"),
        "gif" => ("image/gif", "image"),
        "wav" => ("audio/wav", "audio"),
        "mp3" => ("audio/mpeg", "audio"),
        "m4a" => ("audio/mp4", "audio"),
        "pdf" => ("application/pdf", "file"),
        _ => ("application/octet-stream", "file"),
    }
}
/// Reserves host references locally so the whole send can be journaled before
/// any attachment bytes leave this desktop. The local paths never go on wire.
fn prepare_attachments(
    files: &[std::path::PathBuf],
    command_id: &str,
) -> Result<(Vec<Value>, Vec<RemoteUpload>), String> {
    if files.len() > 20 {
        return Err("Too many attachments (20 maximum)".into());
    }
    let mut references = Vec::new();
    let mut uploads = Vec::new();
    for path in files {
        let metadata =
            std::fs::metadata(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }
        let size = metadata.len();
        if size > MAX_ATTACHMENT_BYTES {
            return Err(format!(
                "{} is too large to send to a remote machine (20 MB maximum)",
                path.display()
            ));
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|name| !name.trim().is_empty() && name.encode_utf16().count() <= 255)
            .ok_or("Invalid attachment file name")?;
        let id = vorkspace_core::remote::protocol::new_attachment_id();
        let (mime, kind) = attachment_type(path);
        references.push(json!({"id":id,"name":name,"size":size,"mimeType":mime,"kind":kind}));
        uploads.push(RemoteUpload {
            command_id: command_id.into(),
            id,
            path: path.clone(),
            size,
        });
    }
    Ok((references, uploads))
}
fn prepare_for_session(
    client: &vorkspace_core::remote::Connections,
    machine: &str,
    environment: &str,
    files: &[std::path::PathBuf],
    command_id: &str,
) -> Result<(Vec<Value>, Vec<RemoteUpload>), String> {
    if files.len() > 20 {
        return Err("Too many attachments (20 maximum)".into());
    }
    let mut references = Vec::new();
    let mut uploads = Vec::new();
    for path in files {
        let source = path.to_string_lossy();
        if vorkspace_core::remote::attachments::is_attachment_source(&source) {
            let source = vorkspace_core::remote::attachments::parse_source(&source)
                .ok_or("Invalid remote attachment")?;
            if source.environment != environment {
                return Err("Attachment belongs to another machine".into());
            }
            let snapshot = client.load_session(machine, &source.session)?;
            let file = snapshot["session"]["blocks"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|block| block["attachments"].as_array().into_iter().flatten())
                .find(|file| {
                    file["id"].as_str() == Some(source.id.as_str())
                        && file["size"].as_u64() == Some(source.size as u64)
                })
                .ok_or("Original remote attachment is no longer available")?;
            let mut reference = json!({});
            for key in ["id", "name", "mimeType", "kind", "size"] {
                reference[key] = file[key].clone();
            }
            references.push(reference);
        } else {
            let (files, pending) = prepare_attachments(std::slice::from_ref(path), command_id)?;
            references.extend(files);
            uploads.extend(pending);
        }
    }
    Ok((references, uploads))
}

fn upload_pending(
    client: &vorkspace_core::remote::Connections,
    machine: &str,
    store: &Database,
    binding: &mut RemoteSession,
) -> Result<(), String> {
    while let Some(upload) = binding.uploads.first().cloned() {
        let mut file = std::fs::File::open(&upload.path)
            .map_err(|e| format!("Cannot read {}: {e}", upload.path.display()))?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.len() != upload.size
            || upload.size > MAX_ATTACHMENT_BYTES
        {
            return Err(
                "Attachment changed before upload. Restore the original file before retrying."
                    .into(),
            );
        }
        let mut offset = 0u64;
        loop {
            let mut data = vec![0; ATTACHMENT_CHUNK_BYTES];
            let read = file.read(&mut data).map_err(|e| e.to_string())?;
            data.truncate(read);
            if read == 0 && offset > 0 {
                break;
            }
            let expected = offset + read as u64;
            if expected > upload.size {
                return Err("Attachment changed during upload".into());
            }
            let reply = client.request(machine,"attachments.upload",json!({"id":upload.id,"offset":offset,"size":upload.size,"data":STANDARD.encode(&data)}))?;
            if reply["offset"].as_u64() != Some(expected) {
                return Err("Host returned an invalid attachment offset".into());
            }
            offset = expected;
            if read == 0 {
                break;
            }
        }
        if offset != upload.size {
            return Err("Attachment changed during upload".into());
        }
        let mut durable = binding.clone();
        durable.uploads.remove(0);
        commit(store, binding, durable)?;
    }
    Ok(())
}
fn accounting(
    snapshot: &Value,
) -> (
    Option<vorkspace_core::context::Usage>,
    Option<vorkspace_core::agent::TurnMetrics>,
) {
    let context = snapshot["session"]
        .get("context")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok());
    let metrics = snapshot["session"]["blocks"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find(|block| block["role"] == "user" && block["draft"] != true)
        .and_then(|block| block.get("turnMetrics"))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok());
    (context, metrics)
}

pub fn question(snapshot: &Value) -> Option<Question> {
    let prompt = &snapshot["session"]["pendingQuestion"];
    let id = prompt["requestId"].as_i64()?;
    let items = prompt["questions"]
        .as_array()?
        .iter()
        .map(|item| QuestionItem {
            id: item["id"].as_str().unwrap_or("").into(),
            prompt: item["prompt"].as_str().unwrap_or("").into(),
            multiple: item["multiSelect"] == true,
            allow_custom: item["allowCustom"] == true,
            options: item["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|option| QuestionOption {
                    id: option["id"].as_str().unwrap_or("").into(),
                    label: option["label"].as_str().unwrap_or("").into(),
                    description: option["description"].as_str().unwrap_or("").into(),
                })
                .collect(),
        })
        .collect();
    Some(Question {
        id: id.to_string(),
        title: prompt["title"].as_str().unwrap_or("Question").into(),
        items,
        auto_resolve_at: prompt["autoResolveAt"].as_u64(),
    })
}
pub fn approvals(snapshot: &Value) -> Vec<Approval> {
    snapshot["session"]["blocks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| {
            let approval = &block["approval"];
            if !approval.is_object()
                || approval
                    .get("decided")
                    .is_some_and(|value| !value.is_null())
            {
                return None;
            }
            Some(Approval {
                id: approval["requestId"].as_i64()?.to_string(),
                title: block["tool"]["title"]
                    .as_str()
                    .or(block["text"].as_str())
                    .unwrap_or("Approval required")
                    .into(),
                detail: block["tool"]["preview"]["title"]
                    .as_str()
                    .or(block["tool"]["detail"].as_str())
                    .unwrap_or("")
                    .into(),
                can_remember: false,
            })
        })
        .collect()
}
fn reply(question: &Question, answers: Vec<Vec<String>>) -> Value {
    let mut selected = serde_json::Map::new();
    let mut custom = serde_json::Map::new();
    for (item, values) in question.items.iter().zip(answers) {
        let mut ids = vec![];
        for value in values {
            if let Some(option) = item
                .options
                .iter()
                .find(|option| option.id == value)
                .or_else(|| item.options.iter().find(|option| option.label == value))
            {
                ids.push(option.id.clone());
            } else if item.allow_custom {
                custom.insert(item.id.clone(), json!(value));
            }
        }
        if !ids.is_empty() {
            selected.insert(item.id.clone(), json!(ids));
        }
    }
    json!({"kind":"answered","answers":selected,"custom":custom})
}
pub(super) fn run(
    input: super::TurnInput,
    controls: std::sync::mpsc::Receiver<Control>,
    store: Database,
    cancel: Arc<AtomicBool>,
    tx: async_channel::Sender<Update>,
) {
    match vorkspace_core::remote::default_connections() {
        Ok(client) => run_with_client(client, input, controls, store, cancel, tx),
        Err(error) => {
            let _ = tx.send_blocking(update(error, Some(TurnOutcome::Failed)));
        }
    }
}
pub(super) fn run_with_client(
    client: Arc<vorkspace_core::remote::Connections>,
    input: super::TurnInput,
    controls: std::sync::mpsc::Receiver<Control>,
    store: Database,
    cancel: Arc<AtomicBool>,
    tx: async_channel::Sender<Update>,
) {
    let super::TurnInput {
        summary,
        prompt,
        mut settings,
        composer_origin,
    } = input;
    let result = (|| -> Result<(), String> {
        if tx.is_closed() {
            return Ok(());
        }
        let operation = operation_lock(&store, summary.id);
        let guard = operation
            .lock()
            .map_err(|_| "Remote session is unavailable")?;
        let mut binding = locked(&store)?
            .remote_session(summary.id)?
            .ok_or("Remote session no longer exists")?;
        let action = settings.remote_action.take();
        let attaching = matches!(action, Some(RemoteAction::Attach(_)));
        if composer_origin.is_some()
            && !matches!(
                action,
                None | Some(RemoteAction::SaveDraft | RemoteAction::Compact)
            )
        {
            return Err("This remote operation cannot consume a composer draft".into());
        }
        if !attaching && Kind::parse(&summary.provider).is_none() {
            return Err(format!(
                "Provider {} is no longer supported. Saved remote history is still available.",
                summary.provider
            ));
        }
        let retrying = action == Some(RemoteAction::Retry);
        if !attaching
            && let Some(recovery) = binding
                .rejected
                .as_ref()
                .filter(|value| value["legacyRecovery"] == true)
        {
            return Err(recovery["error"]
                .as_str()
                .unwrap_or("Older pending requests need recovery in the original desktop app.")
                .into());
        }
        let machine = binding.machine(&client)?;
        if !attaching && !retrying {
            if binding.summary.provider != summary.provider {
                return Err(
                    "Remote provider changed. Refresh the conversation before sending.".into(),
                );
            }
            if !binding.outbox.is_empty() {
                return Err("A remote command needs confirmation. Choose Retry before sending another message.".into());
            }
            if composer_origin.is_some() && binding.composer_origin == composer_origin {
                return Err("The Host journal already accepted this composer version".into());
            }
            if settings.operator || settings.orchestrator {
                return Err(
                    "Operator and orchestration are not available on remote projects".into(),
                );
            }
            if settings
                .model
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                let kind = Kind::parse(&summary.provider).ok_or("Unknown remote provider")?;
                let models = client.model_catalog(&binding.project)?.models_for(kind)?;
                if let Some(model) = models
                    .iter()
                    .find(|model| model.default)
                    .or_else(|| models.first())
                {
                    settings.model = Some(model.id.clone());
                    if settings.effort.is_none() {
                        settings.effort = model.default_effort.clone();
                    }
                    for control in &model.controls {
                        settings
                            .model_settings
                            .entry(control.id.clone())
                            .or_insert_with(|| control.current.clone());
                    }
                }
            }
            let descriptor = client.request(&machine.id, "environment.describe", json!({}))?;
            if !descriptor["providers"].as_array().is_some_and(|providers| {
                providers
                    .iter()
                    .any(|provider| provider.as_str() == Some(&summary.provider))
            }) {
                return Err(format!(
                    "{} is not installed on this machine",
                    summary.provider
                ));
            }
            if vorkspace_core::ticket_context::has_links(&prompt) {
                require_ticket_context(&descriptor)?;
            }
            let ticket_context = if matches!(
                action,
                None | Some(RemoteAction::SendDraft(_) | RemoteAction::BuildPlan(_))
            ) {
                let directory = locked(&store)?.data_dir().map(std::path::Path::to_path_buf);
                Some(vorkspace_core::ticket_context::admit(
                    &prompt,
                    directory.as_deref(),
                    std::path::Path::new(&summary.cwd),
                    &cancel,
                    settings.ticket_context.take(),
                )?)
                .filter(|admission| admission.has_context())
            } else {
                None
            };
            if ticket_context.is_some() {
                require_ticket_context(&descriptor)?;
            }
            let before = binding.clone();
            if binding.session_id.is_none() {
                let mut create = command("create", None);
                create["projectId"] = json!(binding.project.project_id);
                create["harness"] = json!(summary.provider);
                create["model"] = json!(host_model(&summary.provider, settings.model.as_deref())?);
                create["modelSettings"] = json!(model_settings(&summary.provider, &settings));
                create["runtimeMode"] = json!(settings.access.id());
                if let Some(path) = &settings.worktree_cwd {
                    let remote = vorkspace_core::remote::parse_remote_path(path)
                        .ok_or("Invalid remote worktree")?;
                    if remote.environment_id != binding.project.environment_id {
                        return Err("Worktree belongs to another machine".into());
                    }
                    create["worktreeCwd"] = json!(remote.host_path);
                }
                binding.outbox.push(create);
            } else {
                let mut configure = command("configure", binding.session_id.as_deref());
                configure["model"] =
                    json!(host_model(&summary.provider, settings.model.as_deref())?);
                configure["modelSettings"] = json!(model_settings(&summary.provider, &settings));
                configure["runtimeMode"] = json!(settings.access.id());
                binding.outbox.push(configure);
            }
            let operation = match &action {
                Some(RemoteAction::Compact) => "compact",
                Some(RemoteAction::SaveDraft) => "draft",
                Some(RemoteAction::RemoveDraft(_)) => "removeDraft",
                None if prompt.trim() == "/compact" => "compact",
                _ => "send",
            };
            let mut send = command(operation, binding.session_id.as_deref());
            if matches!(operation, "send" | "draft") {
                send["text"] = json!(prompt);
                if let Some(context) = &ticket_context {
                    let prepared = context.append(&prompt);
                    if prepared.len() > 5 * 1024 * 1024 {
                        return Err(
                            "Remote ticket input exceeds 5 MiB. Send fewer ticket references."
                                .into(),
                        );
                    }
                    send["preparedText"] = json!(prepared);
                }
                let (references, uploads) = prepare_for_session(
                    &client,
                    &machine.id,
                    &binding.project.environment_id,
                    &settings.attachments,
                    send["commandId"]
                        .as_str()
                        .ok_or("Remote command identity is missing")?,
                )?;
                send["attachments"] = json!(references);
                binding.uploads.extend(uploads);
                if settings.plan {
                    send["intent"] = json!("plan");
                }
                match &action {
                    Some(RemoteAction::SendDraft(id)) => {
                        send["draftBlockId"] = json!(id);
                        send["intent"] = json!("default");
                    }
                    Some(RemoteAction::BuildPlan(id)) => {
                        send["planBlockId"] = json!(id);
                        send["intent"] = json!("build");
                    }
                    _ => {}
                }
            } else if let Some(RemoteAction::RemoveDraft(id)) = &action {
                send["draftBlockId"] = json!(id);
            }
            binding.outbox.push(send);
            binding.rejected = None;
            binding.summary.has_messages = true;
            binding.summary.updated_at = vorkspace_core::time::now_ms();
            if let Some(origin) = composer_origin.as_ref() {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Stopped".into());
                }
                binding.composer_origin = Some(origin.clone());
            }
            binding = locked(&store)?.commit_remote_journal(&before, binding)?;
            if let Some(origin) = composer_origin {
                // The outbox can continue independently of the observing window.
                let _ = tx.send_blocking(Update {
                    transition: TurnTransition::AcceptedComposer(origin),
                    status: "Sending to Host…".into(),
                    ..Default::default()
                });
            }
        }
        let mut run = if let Some(RemoteAction::Attach(run_id)) = action {
            Some(ObservedRun {
                run_id,
                legacy_prompt: None,
            })
        } else {
            upload_pending(&client, &machine.id, &store, &mut binding)?;
            dispatch_outbox(&client, &machine.id, &store, &mut binding)?
        };
        drop(guard);
        let id = binding
            .session_id
            .clone()
            .ok_or("No remote session has been started")?;
        let mut observation = SessionObservation::default();
        let mut history = RemoteHistory::default();
        let mut activity = super::activity::ToolActivity::default();
        let mut seen_approvals = HashSet::new();
        let mut seen_questions = HashSet::new();
        let mut current_question = None;
        let mut stop_sent = false;
        loop {
            // Cancel is explicit Stop. Dropping the desktop receiver simply
            // ends observation, leaving the host process and run untouched.
            if tx.is_closed() {
                return Ok(());
            }
            if let Some(first) = client.poll_session(&machine.id, &id, &mut observation)? {
                let snapshot = observation
                    .snapshot()
                    .ok_or("Host did not return a session snapshot")?;
                if run.as_mut().is_some_and(|run| !run.accepts(snapshot)) {
                    tx.send_blocking(update(
                        "Refreshing conversation…",
                        Some(TurnOutcome::Superseded),
                    ))
                    .map_err(|_| "Window closed")?;
                    break;
                }
                let host_running = snapshot["status"] == "running";
                let running = host_running && run.is_some();
                let current_approvals = approvals(snapshot);
                current_question = question(snapshot);
                let mut next = if running {
                    update("Working…", None)
                } else if snapshot["status"] == "interrupted" {
                    update(
                        "Host interrupted this turn. Inspect its work before continuing.",
                        Some(TurnOutcome::Failed),
                    )
                } else if stop_sent {
                    update("Ready", Some(TurnOutcome::Cancelled))
                } else {
                    update("Ready", Some(TurnOutcome::Completed))
                };
                let (start, rows) =
                    history.update(snapshot, first, &binding.project.environment_id, &id)?;
                activity.snapshot(history.rows(), start, &binding.project.cwd);
                next.transcript = if running {
                    TranscriptPatch::Snapshot { start, rows }
                } else {
                    // Stop can skip queued live patches. Settlement cannot depend
                    // on the view having applied any of them.
                    TranscriptPatch::Snapshot {
                        start: 0,
                        rows: std::mem::take(&mut history).into_rows(),
                    }
                };
                next.activity = activity.label();
                next.binding = ProviderBinding::Set(
                    snapshot["session"]["providerSessionId"]
                        .as_str()
                        .map(str::to_owned),
                );
                next.model = snapshot["session"]["model"].as_str().map(str::to_owned);
                (next.context, next.metrics) = accounting(snapshot);
                let current_ids: HashSet<_> =
                    current_approvals.iter().map(|a| a.id.clone()).collect();
                next.approvals_resolved =
                    seen_approvals.difference(&current_ids).cloned().collect();
                next.approvals = current_approvals
                    .into_iter()
                    .filter(|a| !seen_approvals.contains(&a.id))
                    .collect();
                seen_approvals = current_ids;
                let question_ids: HashSet<_> =
                    current_question.iter().map(|q| q.id.clone()).collect();
                next.questions_resolved =
                    seen_questions.difference(&question_ids).cloned().collect();
                next.questions = current_question
                    .iter()
                    .filter(|q| !seen_questions.contains(&q.id))
                    .cloned()
                    .collect();
                seen_questions = question_ids;
                locked(&store)?.observe_remote_activity(
                    &binding,
                    snapshot,
                    !seen_approvals.is_empty() || !seen_questions.is_empty(),
                )?;
                tx.send_blocking(next).map_err(|_| "Window closed")?;
                if !running {
                    break;
                }
            }
            let active = run.as_ref().ok_or("Host did not start a turn")?;
            if cancel.load(Ordering::Relaxed) && !stop_sent {
                let mut stop = command("cancel", Some(&id));
                stop["runId"] = json!(active.id()?);
                dispatch_control(&client, &machine.id, &store, &mut binding, stop)?;
                stop_sent = true;
            }
            while !cancel.load(Ordering::Relaxed) {
                let Ok(control) = controls.try_recv() else {
                    break;
                };
                let mut next = command("approve", Some(&id));
                next["runId"] = json!(active.id()?);
                next["requestId"] = json!(
                    control
                        .id
                        .parse::<i64>()
                        .map_err(|_| "Invalid host request ID")?
                );
                match control.decision {
                    Decision::Allow | Decision::Session => next["decision"] = json!("allow"),
                    Decision::Deny
                        if current_question
                            .as_ref()
                            .is_some_and(|q| q.id == control.id) =>
                    {
                        next["type"] = json!("answer");
                        next["reply"] = json!({"kind":"skipped"});
                    }
                    Decision::Deny => next["decision"] = json!("deny"),
                    Decision::Answer(answers) => {
                        let question = current_question
                            .as_ref()
                            .filter(|q| q.id == control.id)
                            .ok_or("Host question is no longer pending")?;
                        next["type"] = json!("answer");
                        next["reply"] = reply(question, answers);
                    }
                    Decision::KeepOpen => continue,
                    Decision::Steer(_) => {
                        return Err("Steering is not supported by this remote host".into());
                    }
                }
                dispatch_control(&client, &machine.id, &store, &mut binding, next)?;
            }
            for _ in 0..15 {
                if tx.is_closed() {
                    return Ok(());
                }
                if cancel.load(Ordering::Relaxed) && !stop_sent {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        // An error preserves optimistic prompts still awaiting a host receipt.
        let _ = tx.send_blocking(update(error, Some(TurnOutcome::Failed)));
    }
}

#[cfg(test)]
mod tests;
