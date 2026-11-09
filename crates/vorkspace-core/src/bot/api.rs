use serde_json::Value;
/// Reject misspelled fields and ambiguous types before any app mutation.
pub fn validate(action: &str, input: &Value) -> Result<(), String> {
    let fields = match action {
        "models.list" | "soul.read" | "habits.list" | "bots.list" => "",
        "bots.read" => "bot before limit maxChars",
        "bots.historyStatus" => "bot",
        "bots.recoverHistory" => "bot expectedHistory confirmStopped",
        "bots.send" => "to message expectReply",
        "bots.configure" => "compactionMode",
        "sessions.list" | "folders.list" | "worktrees.list" => "project",
        "sessions.read" => "sessionId project before limit maxChars",
        "sessions.start" => {
            "prompt project harness model modelSettings runtimeMode title draft reveal workspaceMode worktreeBase worktreeCwd notifyOnComplete placement besideSessionId"
        }
        "sessions.send" => "sessionId prompt project notifyOnComplete",
        "sessions.draft" => "sessionId prompt project",
        "sessions.stop" | "sessions.archive" | "sessions.delete" => "sessionId project",
        "workspace.list" => "project",
        "codebase.tools" | "codebase.status" | "codebase.index" => "project worktreeCwd",
        "codebase.configure" => "project enabled worktreeCwd",
        "codebase.query" => "project worktreeCwd tool arguments",
        "workspace.open" => {
            "project kind sessionId noteId provider itemKey bot document path placement targetPaneId"
        }
        "workspace.focus" => "project paneId",
        "workspace.close" => "project paneId confirmRunning expectedProcess",
        "workspace.move" => "project paneId tabId targetPaneId edge",
        "workspace.detach" => "project paneId index",
        "workspace.reorder" => "project tabId index",
        "workspace.resize" => "project tabId sashPath index ratio",
        "worktrees.create" => "branch base existing project",
        "worktrees.remove" => "path project",
        "folders.move" => "sessionId folderId newFolderName project",
        "notes.list" => "project limit offset",
        "notes.read" => "project id noteId",
        "notes.write" => "project id noteId title body tags",
        "todos.list" => "project",
        "todos.add" => "title description attachments dependencies status github project",
        "todos.update" => "id title description attachments dependencies status before project",
        "todos.remove" => "id project",
        "todos.publish" | "todos.reconcile" => "id project",
        "todos.retry" => "id project confirmNoIssue",
        "todos.reconcileEdit" => "id project",
        "todos.retryEdit" => "id project confirmNoEffect",
        "todos.discardEdit" => "id project confirmChecked",
        "todos.cancelPublish" => "id project confirmNoIssue",
        "minions.hire" => "name role specialties project",
        "minions.fire" => "minion",
        "minions.retry" | "minions.discard" => "minion messageId",
        "minions.spawn" => "prompt name role specialties project harness model",
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
        // `before` is a to-do's ID here and a page cursor everywhere else.
        let todo_before = action == "todos.update" && key == "before";
        if [
            "draft",
            "reveal",
            "notifyOnComplete",
            "existing",
            "enabled",
            "expectReply",
            "github",
            "confirmNoIssue",
            "confirmNoEffect",
            "confirmChecked",
            "confirmRunning",
            "confirmStopped",
        ]
        .contains(&key.as_str())
            && !value.is_boolean()
        {
            return Err(format!("{key} must be true or false"));
        }
        if !todo_before
            && ["before", "limit", "offset", "maxChars", "number", "index"].contains(&key.as_str())
            && value.as_u64().is_none()
        {
            return Err(format!("{key} must be a non-negative integer"));
        }
        if (todo_before
            || ![
                "draft",
                "reveal",
                "notifyOnComplete",
                "existing",
                "enabled",
                "expectReply",
                "github",
                "confirmNoIssue",
                "confirmNoEffect",
                "confirmChecked",
                "confirmRunning",
                "confirmStopped",
                "before",
                "limit",
                "offset",
                "maxChars",
                "number",
                "index",
                "id",
                "sessionId",
                "besideSessionId",
                "paneId",
                "tabId",
                "targetPaneId",
                "sashPath",
                "ratio",
                "arguments",
                "modelSettings",
                "schedule",
                "tags",
                "specialties",
                "options",
                "members",
                "to",
                "attachments",
                "dependencies",
            ]
            .contains(&key.as_str()))
            && !value.is_string()
        {
            return Err(format!("{key} must be text"));
        }
    }
    if input
        .get("arguments")
        .is_some_and(|value| !value.is_object())
    {
        return Err("arguments must be an object".into());
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
    if action.starts_with("workspace.") {
        validate_workspace(action, input)?;
    }
    if action.starts_with("notes.") {
        if input.get("id").is_some() && input.get("noteId").is_some() {
            return Err("Use id or noteId, not both".into());
        }
        if action == "notes.read" && input.get("id").is_none() && input.get("noteId").is_none() {
            return Err("Provide id or noteId".into());
        }
    }
    if input.get("tags").is_some_and(|v| {
        v.as_array().is_none_or(|a| {
            a.len() > 32 || a.iter().any(|v| v.as_str().is_none_or(|s| s.len() > 160))
        })
    }) {
        return Err("tags must be an array of up to 32 short strings".into());
    }
    for (key, limit) in [
        ("attachments", crate::todos::MAX_ATTACHMENTS),
        ("dependencies", crate::todos::MAX_DEPENDENCIES),
    ] {
        if input.get(key).is_some_and(|value| {
            value.as_array().is_none_or(|values| {
                values.len() > limit
                    || values.iter().any(|value| {
                        value
                            .as_str()
                            .is_none_or(|text| text.trim().is_empty() || text.len() > 4096)
                    })
            })
        }) {
            return Err(format!(
                "{key} must be an array of up to {limit} nonempty references"
            ));
        }
    }
    if action.starts_with("todos.")
        && input.get("status").is_some_and(|value| {
            value
                .as_str()
                .and_then(crate::todos::TodoStatus::from_id)
                .is_none()
        })
    {
        return Err("status must be todo, inProgress, inReview or done".into());
    }
    if input.get("specialties").is_some_and(|v| {
        v.as_array().is_none_or(|a| {
            a.len() > 32
                || a.iter().any(|v| {
                    v.as_str()
                        .is_none_or(|s| s.trim().is_empty() || s.len() > 160)
                })
        })
    }) {
        return Err("specialties must be an array of up to 32 nonempty short strings".into());
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
    if action == "bots.configure"
        && !matches!(
            input["compactionMode"].as_str(),
            Some("sessionRollover" | "native")
        )
    {
        return Err("compactionMode must be sessionRollover or native".into());
    }
    if action == "bots.recoverHistory"
        && (input["confirmStopped"].as_bool() != Some(true)
            || input["expectedHistory"].as_str().is_none_or(str::is_empty))
    {
        return Err("Stop or close the external CLI, then provide confirmStopped:true and expectedHistory from bots.historyStatus".into());
    }
    Ok(())
}

fn validate_workspace(action: &str, input: &Value) -> Result<(), String> {
    for key in ["paneId", "tabId", "targetPaneId"] {
        if input.get(key).is_some_and(|value| value.as_i64().is_none()) {
            return Err(format!("{key} must be an integer pane ID"));
        }
    }
    if input.get("sashPath").is_some_and(|value| {
        value
            .as_array()
            .is_none_or(|path| path.len() > 32 || path.iter().any(|part| part.as_u64().is_none()))
    }) {
        return Err("sashPath must be an array of non-negative child indices".into());
    }
    if input.get("ratio").is_some_and(|value| {
        value
            .as_f64()
            .is_none_or(|ratio| !ratio.is_finite() || !(0.0..=1.0).contains(&ratio))
    }) {
        return Err("ratio must be a number between 0 and 1".into());
    }
    match action {
        "workspace.open" => {
            let kind = input
                .get("kind")
                .and_then(Value::as_str)
                .ok_or("Provide kind")?;
            let required = match kind {
                "session" => Some("sessionId"),
                "note" => Some("noteId"),
                "connector" => Some("provider"),
                "item" => Some("itemKey"),
                "bot" | "botDocument" => Some("bot"),
                "file" => Some("path"),
                "notes" | "todos" | "automations" | "terminal" | "search" | "codebase" => None,
                _ => return Err("Unknown workspace pane kind".into()),
            };
            if let Some(key) = required {
                if input.get(key).is_none() {
                    return Err(format!("Provide {key}"));
                }
            }
            if kind == "botDocument" && input.get("document").is_none() {
                return Err("Provide document".into());
            }
            let placement = input
                .get("placement")
                .and_then(Value::as_str)
                .unwrap_or("tab");
            if (placement == "tab") != input.get("targetPaneId").is_none() {
                return Err("targetPaneId requires placement right or down, and split placement requires targetPaneId".into());
            }
            let allowed = match kind {
                "session" => &["sessionId"][..],
                "note" => &["noteId"],
                "connector" => &["provider"],
                "item" => &["itemKey"],
                "bot" => &["bot"],
                "botDocument" => &["bot", "document"],
                "file" => &["path"],
                _ => &[],
            };
            if let Some(extra) = input.as_object().unwrap().keys().find(|key| {
                !["project", "kind", "placement", "targetPaneId"].contains(&key.as_str())
                    && !allowed.contains(&key.as_str())
            }) {
                return Err(format!("{kind} panes do not take {extra}"));
            }
        }
        "workspace.focus" | "workspace.close" | "workspace.detach" => {
            if input.get("paneId").is_none() {
                return Err("Provide paneId".into());
            }
        }
        "workspace.move" => {
            if input.get("paneId").is_some() == input.get("tabId").is_some() {
                return Err("Provide exactly one of paneId or tabId".into());
            }
            if input.get("targetPaneId").is_none() {
                return Err("Provide targetPaneId".into());
            }
            if !matches!(
                input.get("edge").and_then(Value::as_str),
                Some("left" | "right" | "top" | "bottom")
            ) {
                return Err("edge must be left, right, top or bottom".into());
            }
        }
        "workspace.reorder" => {
            if input.get("tabId").is_none() || input.get("index").is_none() {
                return Err("Provide tabId and index".into());
            }
        }
        "workspace.resize"
            if ["tabId", "sashPath", "index", "ratio"]
                .iter()
                .any(|key| input.get(*key).is_none()) =>
        {
            return Err("Provide tabId, sashPath, index and ratio".into());
        }
        _ => (),
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
            | "bots.historyStatus"
            | "sessions.list"
            | "sessions.read"
            | "codebase.tools"
            | "codebase.status"
            | "codebase.query"
            | "workspace.list"
            | "folders.list"
            | "notes.list"
            | "notes.read"
            | "todos.list"
            | "minions.list"
            | "worktrees.list"
            | "artifacts.list"
            | "artifacts.read"
            | "soul.read"
            | "memory.read"
            | "memory.search"
            | "habits.list"
    )
}

/// What a minion may do with its parent's capability: share memory and the
/// board, read, and take notes; never start work or change its parent.
pub fn minion_allowed(action: &str) -> bool {
    action.starts_with("memory.")
        || action.starts_with("todos.")
        || action.starts_with("notes.")
        || matches!(
            action,
            "bots.list"
                | "bots.read"
                | "bots.send"
                | "models.list"
                | "soul.read"
                | "sessions.list"
                | "sessions.read"
                | "codebase.tools"
                | "codebase.status"
                | "codebase.query"
                | "workspace.list"
                | "worktrees.list"
                | "minions.list"
                | "artifacts.list"
                | "artifacts.read"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codebase_actions_preserve_read_and_write_capabilities() {
        assert!(validate(
            "codebase.query",
            &json!({"tool":"search_graph","arguments":{"query":"main"}})
        )
        .is_ok());
        assert!(validate(
            "codebase.query",
            &json!({"tool":"search_graph","arguments":"bad"})
        )
        .is_err());
        assert!(validate("codebase.configure", &json!({"enabled":true})).is_ok());
        assert!(validate("workspace.open", &json!({"kind":"codebase"})).is_ok());
        for action in ["codebase.status", "codebase.tools", "codebase.query"] {
            assert!(is_read_only(action));
            assert!(minion_allowed(action));
        }
        for action in ["codebase.index", "codebase.configure"] {
            assert!(!is_read_only(action));
            assert!(!minion_allowed(action));
        }
    }
    #[test]
    fn native_recovery_requires_explicit_confirmation_and_is_not_a_minion_action() {
        assert!(is_read_only("bots.historyStatus"));
        assert!(!is_read_only("bots.recoverHistory"));
        assert!(!minion_allowed("bots.recoverHistory"));
        assert!(validate(
            "bots.recoverHistory",
            &json!({"bot":"Helper","expectedHistory":"snapshot","confirmStopped":true})
        )
        .is_ok());
        for input in [
            json!({"bot":"Helper","expectedHistory":"snapshot"}),
            json!({"bot":"Helper","expectedHistory":"snapshot","confirmStopped":false}),
            json!({"bot":"Helper","expectedHistory":"snapshot","confirmStopped":"true"}),
            json!({"bot":"Helper","confirmStopped":true}),
        ] {
            assert!(validate("bots.recoverHistory", &input).is_err());
        }
    }
    #[test]
    fn removing_a_worktree_needs_a_path_and_is_a_mutation() {
        assert!(validate("worktrees.remove", &json!({"path":"/tree","project":"/p"})).is_ok());
        assert!(validate("worktrees.remove", &json!({"path":"/tree","force":true})).is_err());
        assert!(!is_read_only("worktrees.remove"));
        assert!(is_read_only("worktrees.list"));
    }
    #[test]
    fn validates_before_mutation() {
        assert!(validate(
            "minions.hire",
            &json!({"name":"Scout","role":"Review","specialties":["Rust"]})
        )
        .is_ok());
        assert!(validate(
            "minions.hire",
            &json!({"name":"Scout","role":"Review","specialties":"Rust"})
        )
        .is_err());
        assert!(validate(
            "minions.hire",
            &json!({"name":"Scout","role":"Review","specialties":[""]})
        )
        .is_err());
        assert!(validate("minions.fire", &json!({"minion":"scout"})).is_ok());
        assert!(!is_read_only("minions.hire") && !is_read_only("minions.fire"));
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
    #[test]
    fn before_is_a_todo_id_or_a_page_cursor_by_action() {
        assert!(validate("todos.update", &json!({"id":"a","before":"b"})).is_ok());
        assert!(validate("todos.update", &json!({"id":"a","before":3})).is_err());
        assert!(validate("bots.read", &json!({"bot":"b","before":3})).is_ok());
        assert!(validate("bots.read", &json!({"bot":"b","before":"3"})).is_err());
        assert!(validate("sessions.read", &json!({"sessionId":1,"before":"3"})).is_err());
    }
    #[test]
    fn workspace_actions_validate_typed_panes_and_layout_coordinates() {
        assert!(validate("workspace.list", &json!({"project":"/p"})).is_ok());
        assert!(is_read_only("workspace.list"));
        assert!(minion_allowed("workspace.list"));
        assert!(!is_read_only("workspace.open"));
        assert!(!minion_allowed("workspace.close"));
        assert!(validate("workspace.open", &json!({"kind":"item","itemKey":"github:repo:pr:1","placement":"right","targetPaneId":-4})).is_ok());
        assert!(validate(
            "workspace.open",
            &json!({"kind":"botDocument","bot":"bot-id","document":"doc-id"})
        )
        .is_ok());
        assert!(validate(
            "workspace.open",
            &json!({"kind":"item","itemKey":"key","bot":"other"})
        )
        .is_err());
        assert!(validate(
            "workspace.open",
            &json!({"kind":"note","noteId":"n","placement":"down"})
        )
        .is_err());
        assert!(validate(
            "workspace.open",
            &json!({"kind":"session","sessionId":1,"targetPaneId":-2})
        )
        .is_err());
        assert!(validate(
            "workspace.move",
            &json!({"paneId":-2,"targetPaneId":1,"edge":"left"})
        )
        .is_ok());
        assert!(validate(
            "workspace.move",
            &json!({"tabId":-2,"targetPaneId":1,"edge":"bottom"})
        )
        .is_ok());
        assert!(validate(
            "workspace.move",
            &json!({"paneId":-2,"tabId":-2,"targetPaneId":1,"edge":"left"})
        )
        .is_err());
        assert!(validate("workspace.move", &json!({"targetPaneId":1,"edge":"left"})).is_err());
        assert!(validate(
            "workspace.move",
            &json!({"paneId":-2,"targetPaneId":1,"edge":"diagonal"})
        )
        .is_err());
        assert!(validate("workspace.detach", &json!({"paneId":-2,"index":0})).is_ok());
        assert!(validate("workspace.reorder", &json!({"tabId":-2,"index":0})).is_ok());
        assert!(validate("workspace.reorder", &json!({"tabId":-2})).is_err());
        assert!(validate(
            "workspace.resize",
            &json!({"tabId":1,"sashPath":[0,1],"index":0,"ratio":0.4})
        )
        .is_ok());
        assert!(validate(
            "workspace.resize",
            &json!({"tabId":1,"sashPath":[-1],"index":0,"ratio":0.4})
        )
        .is_err());
        assert!(validate(
            "workspace.resize",
            &json!({"tabId":1,"sashPath":[],"index":0,"ratio":1.2})
        )
        .is_err());
        assert!(validate(
            "workspace.close",
            &json!({"paneId":-2,"confirmRunning":true})
        )
        .is_ok());
        assert!(validate(
            "workspace.close",
            &json!({"paneId":-2,"confirmRunning":true,"expectedProcess":"vim"})
        )
        .is_ok());
    }

    #[test]
    fn native_note_ids_and_legacy_ids_are_mutually_exclusive() {
        assert!(validate("notes.read", &json!({"noteId":"native-note"})).is_ok());
        assert!(validate("notes.read", &json!({"id":42})).is_ok());
        assert!(validate("notes.read", &json!({})).is_err());
        assert!(validate(
            "notes.write",
            &json!({"noteId":"native-note","title":"T","body":"B"})
        )
        .is_ok());
        assert!(validate(
            "notes.write",
            &json!({"id":42,"noteId":"native-note","title":"T","body":"B"})
        )
        .is_err());
    }
}
