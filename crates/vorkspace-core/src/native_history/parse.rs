use super::*;

fn time(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        chrono::DateTime::parse_from_rfc3339(value.as_str()?)
            .ok()?
            .timestamp_millis()
            .try_into()
            .ok()
    })
}
fn text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.into();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| match part["type"].as_str()? {
            "text" | "Text" | "input_text" | "output_text" => {
                part["text"].as_str().map(str::to_owned)
            }
            "image" | "Image" | "input_image" => {
                Some("[Image attached in the external CLI]".into())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub(super) fn codex(
    value: &Value,
    binding: &Binding,
    cursor: &mut Cursor,
    turns: &mut Vec<Turn>,
) -> Result<(), String> {
    let data = &value["payload"];
    let at = time(&value["timestamp"]);
    match value["type"].as_str() {
        Some("compacted") => {
            cursor.compaction = Some(format!("{}:{}", binding.thread, value["timestamp"]));
            cursor.context = Some(crate::context::Usage::default());
        }
        Some("session_meta") => {
            if data["id"].as_str() != Some(&binding.thread) {
                return Err("Codex transcript identity changed".into());
            }
            cursor.verified = true;
        }
        Some("turn_context") => {
            if let Some(turn) = &mut cursor.pending {
                if data["turn_id"].as_str().is_none_or(|id| id == turn.id) {
                    turn.model = data["model"].as_str().map(str::to_owned);
                }
            }
        }
        Some("event_msg") => match data["type"].as_str() {
            Some("token_count") => {
                if let Some(used) = data["info"]["last_token_usage"]["total_tokens"].as_u64() {
                    cursor.context = Some(crate::context::Usage {
                        used,
                        window: data["info"]["model_context_window"]
                            .as_u64()
                            .filter(|v| *v > 0),
                    });
                }
            }
            Some("task_started") => {
                let id = data["turn_id"]
                    .as_str()
                    .ok_or("Native Codex turn has no identity")?;
                if let Some(previous) = cursor.pending.take() {
                    if previous.id == id {
                        cursor.pending = Some(previous);
                        return Ok(());
                    }
                    // A new task boundary also closes a CLI turn interrupted by
                    // process death. Preserve its recorded messages, never replay it.
                    turns.push(finish_codex(previous, at));
                }
                cursor.pending = Some(Turn {
                    id: id.into(),
                    started_at: time(&data["started_at"]).or(at),
                    ..Default::default()
                });
            }
            Some("item_completed") => {
                let Some(turn) = &mut cursor.pending else {
                    return Ok(());
                };
                if data["turn_id"].as_str().is_some_and(|id| id != turn.id) {
                    return Ok(());
                }
                let item = &data["item"];
                let role = match item["type"].as_str() {
                    Some("UserMessage" | "userMessage") => "user",
                    Some("AgentMessage" | "agentMessage") => "assistant",
                    _ => return Ok(()),
                };
                let id = item["id"]
                    .as_str()
                    .ok_or("Native message has no identity")?;
                if turn.item_ids.insert(id.into()) {
                    let body = if item["text"].is_string() {
                        text(&item["text"])
                    } else {
                        text(&item["content"])
                    };
                    turn.push(role, body, at, None)?;
                }
            }
            Some("user_message" | "agent_message") => {
                if let Some(turn) = &mut cursor.pending {
                    let role = if data["type"] == "user_message" {
                        "user"
                    } else {
                        "assistant"
                    };
                    // Older CLIs emit these events instead of item_completed.
                    let message = Message {
                        role: role.into(),
                        text: data["message"].as_str().unwrap_or_default().into(),
                        at,
                        claude_uuid: None,
                        codex_receipt: None,
                    };
                    turn.push_event(message)?;
                }
            }
            Some("task_complete" | "task_completed" | "turn_aborted") => {
                if let Some(mut turn) = cursor.pending.take() {
                    if data["turn_id"].as_str().is_some_and(|id| id != turn.id) {
                        cursor.pending = Some(turn);
                        return Ok(());
                    }
                    if !turn
                        .messages
                        .iter()
                        .chain(&turn.event_messages)
                        .any(|m| m.role == "assistant")
                    {
                        if let Some(last) = data["last_agent_message"].as_str() {
                            turn.push("assistant", last.into(), at, None)?;
                        }
                    }
                    turns.push(finish_codex(turn, time(&data["completed_at"]).or(at)));
                }
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}
pub(super) fn finish_codex(mut turn: Turn, at: Option<u64>) -> Turn {
    if turn.messages.is_empty() {
        turn.messages = std::mem::take(&mut turn.event_messages);
    } else if !turn.messages.iter().any(|m| m.role == "user") {
        let mut users = turn
            .event_messages
            .iter()
            .filter(|m| m.role == "user")
            .cloned()
            .collect::<Vec<_>>();
        users.append(&mut turn.messages);
        turn.messages = users;
    }
    if !turn.messages.iter().any(|m| m.role == "assistant") {
        turn.messages.extend(
            turn.event_messages
                .drain(..)
                .filter(|m| m.role == "assistant"),
        );
    }
    turn.event_messages.clear();
    turn.budget = None;
    turn.item_ids.clear();
    turn.completed_at = at;
    turn.complete = true;
    turn
}

pub(super) fn claude(
    value: &Value,
    binding: &Binding,
    cursor: &mut Cursor,
    turns: &mut Vec<Turn>,
) -> Result<(), String> {
    if let Some(id) = value["sessionId"].as_str() {
        if id != binding.thread {
            return Err("Claude transcript identity changed".into());
        }
        cursor.verified = true;
    }
    if value["isSidechain"] == true || value["isMeta"] == true {
        return Ok(());
    }
    if value["type"] == "system" && value["subtype"] == "compact_boundary" {
        cursor.compaction = Some(format!("{}:{}", binding.thread, value["timestamp"]));
        cursor.context = Some(crate::context::Usage::default());
    }
    let kind = value["type"].as_str().unwrap_or_default();
    if !matches!(kind, "user" | "assistant") {
        return Ok(());
    }
    let content = &value["message"]["content"];
    let tool_result = content
        .as_array()
        .is_some_and(|a| a.iter().any(|p| p["type"] == "tool_result"));
    let at = time(&value["timestamp"]);
    let id = value["uuid"]
        .as_str()
        .ok_or("Native Claude message has no identity")?;
    if kind == "user" && !tool_result {
        if let Some(mut turn) = cursor.pending.take() {
            if !turn.messages.is_empty() {
                turn.completed_at = turn.completed_at.or(at);
                turns.push(turn);
            }
        }
        cursor.pending = Some(Turn {
            id: id.into(),
            started_at: at,
            ..Default::default()
        });
        cursor
            .pending
            .as_mut()
            .unwrap()
            .push("user", text(content), at, Some(id))?;
    } else if kind == "assistant" {
        if let Some(used) = crate::context::claude_used(&value["message"]["usage"]) {
            cursor
                .context
                .get_or_insert_with(Default::default)
                .merge(Some(used), None);
        }
        let Some(turn) = &mut cursor.pending else {
            return Ok(());
        };
        // The provider's main-chain UUIDs distinguish final blocks from tool
        // results, sidechains and reasoning; none of those become user prose.
        if turn.item_ids.insert(id.into()) {
            turn.push("assistant", text(content), at, Some(id))?;
        }
        turn.model = value["message"]["model"]
            .as_str()
            .map(str::to_owned)
            .or(turn.model.take());
        turn.complete = matches!(
            value["message"]["stop_reason"].as_str(),
            Some("end_turn" | "stop_sequence" | "max_tokens")
        );
        if turn.complete {
            turn.completed_at = at;
        }
    }
    if let Some(turn) = cursor
        .pending
        .as_mut()
        .filter(|turn| !turn.messages.is_empty())
    {
        turn.claude_records.push(ClaudeRecord {
            uuid: id.into(),
            messages: turn.messages.len(),
        });
    }
    Ok(())
}
