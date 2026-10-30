use serde_json::Value;
/// Reject misspelled fields and ambiguous types before any app mutation.
pub fn validate(action: &str, input: &Value) -> Result<(), String> {
    let fields = match action {
        "models.list" | "soul.read" | "habits.list" | "channels.list" | "bots.list" => "",
        "bots.read" => "bot before limit maxChars",
        "bots.send" => "to message expectReply",
        "channels.create" => "name topic members",
        "channels.read" => "channel limit",
        "channels.post" => "channel message to",
        "sessions.list" | "folders.list" | "worktrees.list" => "project",
        "sessions.read" => "sessionId project before limit maxChars",
        "sessions.start" => {
            "prompt project harness model modelSettings runtimeMode title draft reveal workspaceMode worktreeBase worktreeCwd notifyOnComplete placement besideSessionId"
        }
        "sessions.send" => "sessionId prompt project notifyOnComplete",
        "sessions.draft" => "sessionId prompt project",
        "sessions.stop" | "sessions.archive" | "sessions.delete" => "sessionId project",
        "worktrees.create" => "branch base existing project",
        "folders.move" => "sessionId folderId newFolderName project",
        "notes.list" => "limit offset",
        "notes.read" => "id",
        "notes.write" => "id title body tags",
        "todos.list" => "project",
        "todos.add" => "title status project",
        "todos.update" => "id title status before project",
        "todos.remove" => "id project",
        "minions.spawn" => "prompt name project harness model",
        "minions.list" => "",
        "artifacts.list" => "kind limit offset",
        "artifacts.read" => "id",
        "artifacts.write" => "id kind title body summary",
        "soul.update" => "text expectedHash",
        "memory.read" => "topic",
        "memory.search" => "query since",
        "memory.add" => "fact topic until",
        "memory.replace" => "find fact topic until",
        "memory.remove" => "find topic",
        "habits.add" => "name instructions schedule",
        "habits.update" => "id name instructions schedule enabled",
        "habits.run" | "habits.remove" => "id",
        "chat.card" => "type repo number note sessionId options name instructions schedule",
        _ => return Err("Unknown app action. Run app help".into()),
    };
    for (key, value) in input.as_object().ok_or("Input must be an object")? {
        if !fields.split_whitespace().any(|f| f == key) {
            return Err(format!("{action} does not take {key}"));
        }
        if [
            "draft",
            "reveal",
            "notifyOnComplete",
            "existing",
            "enabled",
            "expectReply",
        ]
        .contains(&key.as_str())
            && !value.is_boolean()
        {
            return Err(format!("{key} must be true or false"));
        }
        if ["before", "limit", "offset", "maxChars", "number"].contains(&key.as_str())
            && value.as_u64().is_none()
        {
            return Err(format!("{key} must be a non-negative integer"));
        }
        if ![
            "draft",
            "reveal",
            "notifyOnComplete",
            "existing",
            "enabled",
            "expectReply",
            "before",
            "limit",
            "offset",
            "maxChars",
            "number",
            "id",
            "sessionId",
            "besideSessionId",
            "modelSettings",
            "schedule",
            "tags",
            "options",
            "members",
            "to",
        ]
        .contains(&key.as_str())
            && !value.is_string()
        {
            return Err(format!("{key} must be text"));
        }
    }
    if input.get("runtimeMode").is_some_and(|v| {
        !matches!(
            v.as_str(),
            Some("read-only" | "supervised" | "auto-accept-edits" | "auto" | "full-access")
        )
    }) {
        return Err("Unknown runtimeMode".into());
    }
    if input
        .get("workspaceMode")
        .is_some_and(|v| !matches!(v.as_str(), Some("current" | "checkout" | "worktree")))
    {
        return Err("workspaceMode must be current or worktree".into());
    }
    if input
        .get("placement")
        .is_some_and(|value| !matches!(value.as_str(), Some("tab" | "right" | "down")))
    {
        return Err("placement must be tab, right or down".into());
    }
    if input.get("besideSessionId").is_some()
        && input
            .get("placement")
            .and_then(Value::as_str)
            .is_none_or(|value| value == "tab")
    {
        return Err("besideSessionId requires placement right or down".into());
    }
    if input.get("tags").is_some_and(|v| {
        v.as_array().is_none_or(|a| {
            a.len() > 32 || a.iter().any(|v| v.as_str().is_none_or(|s| s.len() > 160))
        })
    }) {
        return Err("tags must be an array of up to 32 short strings".into());
    }
    for key in ["members", "to"]
        .into_iter()
        .filter(|key| *key != "to" || action != "bots.send")
    {
        if input.get(key).is_some_and(|v| {
            v.as_array().is_none_or(|a| {
                a.len() > 32
                    || a.iter()
                        .any(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 160))
            })
        }) {
            return Err(format!("{key} must be an array of up to 32 Bot names"));
        }
    }
    if action == "bots.send" && input.get("to").is_some_and(|v| !v.is_string()) {
        return Err("to must be a Bot name or ID".into());
    }
    Ok(())
}
/// Read queries do not reserve durable effects or turn a frequent listing into
/// a database write. Every other validated action requires durable admission.
pub fn is_read_only(action: &str) -> bool {
    matches!(
        action,
        "models.list"
            | "bots.list"
            | "bots.read"
            | "sessions.list"
            | "sessions.read"
            | "folders.list"
            | "notes.list"
            | "notes.read"
            | "todos.list"
            | "minions.list"
            | "worktrees.list"
            | "artifacts.list"
            | "artifacts.read"
            | "channels.list"
            | "channels.read"
            | "soul.read"
            | "memory.read"
            | "memory.search"
            | "habits.list"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn validates_before_mutation() {
        assert!(validate(
            "bots.send",
            &json!({"to":"Beta","message":"Review","expectReply":true})
        )
        .is_ok());
        assert!(validate("bots.send", &json!({"to":["Beta"],"message":"Review"})).is_err());
        assert!(validate("bots.send", &json!({"to":"Beta","expectReply":"true"})).is_err());
        assert!(
            is_read_only("bots.read") && is_read_only("bots.list") && !is_read_only("bots.send")
        );
        assert!(validate("sessions.start", &json!({"prompt":"x","draft":"false"})).is_err());
        assert!(validate("sessions.delete", &json!({"sessionId":1,"force":true})).is_err());
        assert!(validate("sessions.start", &json!({"runtimeMode":"unsafe"})).is_err());
        assert!(validate(
            "sessions.start",
            &json!({"prompt":"x","project":"/code","draft":true})
        )
        .is_ok());
    }
}
