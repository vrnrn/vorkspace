//! Antigravity ACP lifecycle. The provider owns filesystem and terminal access;
//! the client advertises only capabilities it implements.
use super::*;
use std::collections::VecDeque;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Initialize,
    Resume,
    Load,
    New,
    Configure,
    Prompt,
}
#[derive(Default)]
pub(super) struct State {
    sequence: u64,
    stage: Stage,
    session: String,
    options: VecDeque<(&'static str, Value)>,
    message: usize,
    setup: Value,
    settings_applied: bool,
    tool_kinds: HashMap<String, String>,
    pending_method: &'static str,
    deadline: Option<Instant>,
    cancelling: bool,
    mode: Option<&'static str>,
}

pub(super) fn configure(request: &Request) -> Result<(), String> {
    if request.kind != Kind::Antigravity {
        return Err("ACP is available only for Antigravity".into());
    }
    Ok(())
}

impl Protocol {
    pub(super) fn acp_cancel(&mut self, stdin: &mut impl Write) -> Result<(), String> {
        self.acp.cancelling = true;
        self.acp.deadline = None;
        for (_, (wire, _, _)) in self.approvals.drain() {
            write_json(
                stdin,
                json!({"jsonrpc":"2.0","id":wire,"result":{"outcome":{"outcome":"cancelled"}}}),
            )?;
        }
        for id in self
            .approvals
            .keys()
            .chain(self.questions.keys())
            .cloned()
            .collect::<Vec<_>>()
        {
            self.respond(
                Control {
                    id,
                    decision: Decision::Deny,
                },
                stdin,
            )?;
        }
        // Complete local cancellation immediately after notifying the provider;
        // ACP does not promise a response to a cancelled prompt.
        self.done = true;
        if self.acp.session.is_empty() {
            return Ok(());
        }
        write_json(
            stdin,
            json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":self.acp.session}}),
        )
    }

    /// ACP deadlines apply to individual requests, never to a quiet stream.
    pub(super) fn acp_tick(&mut self, stdin: &mut impl Write) -> Result<Vec<Event>, String> {
        if self
            .acp
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.acp.deadline = None;
            let method = self.acp.pending_method;
            self.acp_failure(
                &json!({"message":format!("{method} timed out")}),
                true,
                stdin,
            )?;
        }
        Ok(Vec::new())
    }

    pub(super) fn acp_active_session(&self) -> Option<&str> {
        (self.acp.stage == Stage::Prompt && !self.acp.session.is_empty())
            .then_some(self.acp.session.as_str())
    }

    fn acp_send(
        &mut self,
        stage: Stage,
        method: &'static str,
        params: Value,
        stdin: &mut impl Write,
    ) -> Result<(), String> {
        self.acp.sequence += 1;
        self.acp.stage = stage;
        self.acp.pending_method = method;
        let seconds = if self.probe {
            Some(12)
        } else {
            match stage {
                Stage::Initialize => Some(12),
                Stage::New | Stage::Load | Stage::Resume => Some(45),
                Stage::Configure => Some(15),
                Stage::Prompt => Some(30 * 60),
            }
        };
        self.acp.deadline = seconds.map(|seconds| Instant::now() + Duration::from_secs(seconds));
        write_json(
            stdin,
            json!({"jsonrpc":"2.0","id":self.acp.sequence,"method":method,"params":params}),
        )
    }
    pub(super) fn acp_resume_warm(&mut self, stdin: &mut impl Write) -> Result<(), String> {
        self.acp.message = 0;
        self.acp.cancelling = false;
        self.acp.options.clear();
        self.acp.settings_applied = false;
        let setup = self.acp.setup.clone();
        self.acp_options(&setup)?;
        self.acp_next(stdin)
    }
    pub(super) fn acp_start(&mut self, stdin: &mut impl Write) -> Result<(), String> {
        let params = json!({"protocolVersion":1,"clientInfo":{"name":"vorkspace","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false,"session":{"configOptions":{"boolean":{}}}}});
        self.acp_send(Stage::Initialize, "initialize", params, stdin)
    }
    fn acp_session(&mut self, stdin: &mut impl Write) -> Result<(), String> {
        let mut params = json!({"cwd":self.request.cwd,"mcpServers":crate::visual_bridge::acp_servers(&self.request.settings.environment)});
        if let Some(id) = &self.request.provider_id {
            params["sessionId"] = id.clone().into();
            return self.acp_send(Stage::Resume, "session/resume", params, stdin);
        }
        self.acp_new(stdin)
    }
    fn acp_new(&mut self, stdin: &mut impl Write) -> Result<(), String> {
        let params = json!({"cwd":self.request.cwd,"mcpServers":crate::visual_bridge::acp_servers(&self.request.settings.environment)});
        self.acp_send(Stage::New, "session/new", params, stdin)
    }
    fn acp_next(&mut self, stdin: &mut impl Write) -> Result<(), String> {
        if let Some((method, params)) = self.acp.options.pop_front() {
            if method == "session/set_config_option"
                && self.acp.setup["configOptions"]
                    .as_array()
                    .is_some_and(|options| {
                        options.iter().any(|option| {
                            config_id(option) == params["configId"].as_str().unwrap_or("")
                                && option.get("currentValue").is_some_and(|value| {
                                    value
                                        .as_str()
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| value.to_string())
                                        == params["value"]
                                            .as_str()
                                            .map(str::to_owned)
                                            .unwrap_or_else(|| params["value"].to_string())
                                })
                        })
                    })
            {
                return self.acp_next(stdin);
            }
            self.acp_send(Stage::Configure, method, params, stdin)
        } else if !self.acp.settings_applied {
            self.acp.settings_applied = true;
            let setup = self.acp.setup.clone();
            self.acp_settings(&setup);
            self.acp_next(stdin)
        } else if let Some(mode) = self.acp.mode.take() {
            self.acp_send(
                Stage::Configure,
                "session/set_mode",
                json!({"sessionId":self.acp.session,"modeId":mode}),
                stdin,
            )
        } else {
            self.acp_send(
                Stage::Prompt,
                "session/prompt",
                json!({"sessionId":self.acp.session,"prompt":self.request.acp_content()?}),
                stdin,
            )
        }
    }
    fn acp_options(&mut self, setup: &Value) -> Result<(), String> {
        let settings = &self.request.settings;
        let planning = settings.plan || settings.access == AccessMode::ReadOnly;
        self.acp.mode = Some(if planning {
            "default"
        } else {
            match settings.access {
                AccessMode::FullAccess => "yolo",
                AccessMode::AutoAcceptEdits => "auto_edit",
                _ => "default",
            }
        });
        // Antigravity accepts only model configuration advertised by its server.
        if let Some(model) = &self.request.model {
            if let Some(option) = setup["configOptions"].as_array().and_then(|config| {
                config
                    .iter()
                    .find(|option| config_id(option) == "model")
                    .or_else(|| {
                        config.iter().find(|option| {
                            option["category"] == "model" && config_id(option) != "provider"
                        })
                    })
            }) {
                self.acp.options.push_back((
                    "session/set_config_option",
                    json!({"sessionId":self.acp.session,"configId":config_id(option),"value":model}),
                ));
            }
        }
        Ok(())
    }
    fn acp_settings(&mut self, setup: &Value) {
        let settings = &self.request.settings;
        let mut selected = settings.model_settings.clone();
        if let Some(effort) = &settings.effort {
            selected
                .entry("effort".into())
                .or_insert_with(|| effort.clone());
        }
        selected
            .entry("fast".into())
            .or_insert_with(|| settings.fast.to_string());
        if let Some(thinking) = settings.thinking {
            selected
                .entry("thinking".into())
                .or_insert_with(|| thinking.to_string());
        }
        let config = setup["configOptions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut resolved = std::collections::HashSet::new();
        for (id, value) in selected {
            let Some(option) = setting_option(&config, &id) else {
                continue;
            };
            let config_id = config_id(option);
            if matches!(config_id, "provider" | "model" | "mode") || !resolved.insert(config_id) {
                continue;
            }
            let mut params =
                json!({"sessionId":self.acp.session,"configId":config_id,"value":value});
            if option["type"] == "boolean" {
                if !matches!(value.as_str(), "true" | "false") {
                    continue;
                }
                params["type"] = "boolean".into();
                params["value"] = (value == "true").into();
            }
            self.acp
                .options
                .push_back(("session/set_config_option", params));
        }
    }
    fn acp_failure(
        &mut self,
        error: &Value,
        timed_out: bool,
        stdin: &mut impl Write,
    ) -> Result<(), String> {
        let kind = self.request.kind;
        let detail = error["message"].as_str().unwrap_or("RPC request failed");
        if self.acp.cancelling {
            self.done = true;
            return Ok(());
        }
        match self.acp.stage {
            Stage::Resume if !timed_out => {
                return self.acp_send(Stage::Load, "session/load", json!({"sessionId":self.request.provider_id,"cwd":self.request.cwd,"mcpServers":crate::visual_bridge::acp_servers(&self.request.settings.environment)}), stdin);
            }
            Stage::Load if !timed_out => return self.acp_new(stdin),
            _ => {}
        }
        // A JSON-RPC failure is not evidence that the user needs to log in.
        Err(format!("{}: {detail}", kind.label()))
    }
    pub(super) fn acp_frame(
        &mut self,
        v: Value,
        stdin: &mut impl Write,
    ) -> Result<Vec<Event>, String> {
        let mut events = Vec::new();
        if let Some(method) = v["method"].as_str() {
            if v.get("id").is_some() {
                if method == "session/request_permission" {
                    let request = &v["params"];
                    let subject = request.get("subject").unwrap_or(request);
                    let tool = request
                        .get("toolCall")
                        .or_else(|| request.get("tool_call"))
                        .or_else(|| subject.get("toolCall"))
                        .or_else(|| subject.get("tool_call"))
                        .or_else(|| request.get("subject"))
                        .unwrap_or(request);
                    let call_id = tool["toolCallId"]
                        .as_str()
                        .or_else(|| tool["tool_call_id"].as_str())
                        .unwrap_or("");
                    let known = self.tools.get(call_id);
                    let kind = tool["kind"]
                        .as_str()
                        .or_else(|| self.acp.tool_kinds.get(call_id).map(String::as_str));
                    let title = match kind {
                        Some("edit" | "delete" | "move") => "Allow file changes?".into(),
                        Some("execute") => "Allow this command?".into(),
                        _ => format!(
                            "Allow {}?",
                            tool["title"]
                                .as_str()
                                .or_else(|| known.map(|t| t.name.as_str()))
                                .unwrap_or("this tool")
                        ),
                    };
                    let detail = known
                        .map(|call| format!("{}\n{}\n{}", call.name, call.input, call.output))
                        .unwrap_or_else(|| serde_json::to_string_pretty(tool).unwrap_or_default());
                    let id = v["id"].to_string();
                    self.approvals
                        .insert(id.clone(), (v["id"].clone(), "acp".into(), request.clone()));
                    let read = matches!(kind, Some("read" | "search"));
                    let planning = self.probe
                        || self.request.settings.plan
                        || self.request.settings.access == AccessMode::ReadOnly;
                    let access = self.request.settings.access;
                    let auto = access == AccessMode::FullAccess
                        || (access == AccessMode::AutoAcceptEdits && kind == Some("edit"));
                    if self.acp.stage != Stage::Prompt || self.acp.cancelling {
                        self.approvals.remove(&id);
                        write_json(
                            stdin,
                            json!({"jsonrpc":"2.0","id":v["id"],"result":{"outcome":{"outcome":"cancelled"}}}),
                        )?;
                        return Ok(events);
                    }
                    if planning || auto {
                        self.respond(
                            Control {
                                id,
                                decision: if (planning && !read) || self.probe {
                                    Decision::Deny
                                } else {
                                    Decision::Allow
                                },
                            },
                            stdin,
                        )?;
                    } else {
                        events.push(Event::Approval(Approval {
                            id,
                            title,
                            detail,
                            can_remember: request["options"].as_array().is_some_and(|options| {
                                options.iter().any(|o| o["kind"] == "allow_always")
                            }),
                        }));
                    }
                } else {
                    write_json(
                        stdin,
                        json!({"jsonrpc":"2.0","id":v["id"],"error":{"code":-32601,"message":format!("Unsupported client method: {method}")}}),
                    )?;
                }
            } else if method == "session/update" && self.acp.stage == Stage::Prompt {
                // session/load may replay the entire history. Only live prompt updates render.
                if v["params"]["sessionId"]
                    .as_str()
                    .is_some_and(|id| id != self.acp.session)
                {
                    return Ok(events);
                }
                let params = &v["params"];
                self.acp_update(params.get("update").unwrap_or(params), &mut events);
            }
            return Ok(events);
        }
        if v["id"].as_u64() != Some(self.acp.sequence) {
            return Ok(events);
        }
        self.acp.deadline = None;
        if self.acp.cancelling {
            self.done = true;
            return Ok(vec![Event::Done]);
        }
        if let Some(error) = v.get("error").filter(|error| !error.is_null()) {
            self.acp_failure(error, false, stdin)?;
            return Ok(events);
        }
        let Some(result) = v.get("result") else {
            return Ok(events);
        };
        match self.acp.stage {
            Stage::Initialize => self.acp_session(stdin)?,
            Stage::New | Stage::Load | Stage::Resume => {
                self.acp.session = result["sessionId"]
                    .as_str()
                    .or_else(|| result["session_id"].as_str())
                    .or_else(|| result["id"].as_str())
                    .or_else(|| {
                        (self.acp.stage != Stage::New)
                            .then_some(self.request.provider_id.as_deref())
                            .flatten()
                    })
                    .filter(|id| !id.is_empty())
                    .ok_or("Provider returned no session id")?
                    .into();
                if self.probe {
                    self.catalog_result = Some(result.clone());
                    self.done = true;
                } else {
                    self.acp.setup = result.clone();
                    events.push(Event::SessionId(self.acp.session.clone()));
                    if let Some(model) = result["models"]["currentModelId"].as_str() {
                        events.push(Event::Model(model.into()));
                    }
                    self.acp_options(result)?;
                    self.acp_next(stdin)?;
                }
            }
            Stage::Configure => {
                if result["configOptions"].is_array() {
                    self.acp.setup["configOptions"] = result["configOptions"].clone();
                }
                self.acp_next(stdin)?;
            }
            Stage::Prompt => {
                if result["stopReason"]
                    .as_str()
                    .is_some_and(|reason| !matches!(reason, "end_turn" | "cancelled"))
                {
                    return Err(format!(
                        "Antigravity ended the turn ({}).",
                        result["stopReason"].as_str().unwrap_or("")
                    ));
                }
                self.done = true;
                events.push(Event::Done);
            }
        }
        Ok(events)
    }
    fn acp_update(&mut self, update: &Value, events: &mut Vec<Event>) {
        let kind = update["sessionUpdate"]
            .as_str()
            .or_else(|| update["session_update"].as_str())
            .or_else(|| update["type"].as_str())
            .unwrap_or("");
        match kind {
            "agent_message_chunk" | "agent_message" => self.text(
                &format!("acp-{}", self.acp.message),
                &content_text(&update["content"]),
                kind == "agent_message",
                events,
            ),
            "agent_thought_chunk" | "agent_thought" => {
                events.push(Event::Reasoning(content_text(&update["content"])))
            }
            "tool_call" | "tool_call_update" | "tool_call_content_chunk" => {
                let tool = update
                    .get("toolCall")
                    .or_else(|| update.get("tool_call"))
                    .unwrap_or(update);
                let Some(id) = tool["toolCallId"]
                    .as_str()
                    .or_else(|| tool["tool_call_id"].as_str())
                    .or_else(|| update["toolCallId"].as_str())
                else {
                    return;
                };
                let is_new = !self.tools.contains_key(id);
                if let Some(kind) = tool["kind"].as_str() {
                    self.acp.tool_kinds.insert(id.into(), kind.into());
                }
                let call = self.tools.entry(id.into()).or_insert_with(|| {
                    ToolCall::new(
                        id,
                        tool["title"]
                            .as_str()
                            .or_else(|| tool["kind"].as_str())
                            .unwrap_or("Tool"),
                    )
                });
                if let Some(title) = tool["title"].as_str() {
                    call.name = title.into();
                }
                // ACP titles are often prose; retain semantic kind/locations
                // separately, so actual provider arguments stay unchanged.
                if let Some(input) = tool.get("rawInput").or_else(|| tool.get("raw_input")) {
                    call.input = input
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| serde_json::to_string_pretty(input).unwrap_or_default());
                }
                let mut presentation = call.display.take().unwrap_or_else(|| json!({}));
                presentation["title"] = json!(call.name);
                if let Some(kind) = self.acp.tool_kinds.get(id) {
                    presentation["kind"] = json!(kind);
                }
                for field in ["locations", "preview"] {
                    if let Some(value) = tool.get(field) {
                        presentation[field] = value.clone();
                    }
                }
                presentation["cwd"] = json!(self.request.cwd);
                call.display = Some(presentation);
                let output = content_text(&tool["content"]);
                if kind == "tool_call_content_chunk" {
                    call.append_output(&output);
                } else if !output.is_empty() {
                    call.output = output;
                }
                if let Some(raw) = tool.get("rawOutput").filter(|_| call.output.is_empty()) {
                    call.output = content_text(raw);
                }
                call.bound_output();
                match tool["status"].as_str() {
                    Some("completed") => call.state = ToolState::Completed,
                    Some("failed") => call.state = ToolState::Failed,
                    Some("cancelled" | "canceled" | "aborted") => call.state = ToolState::Cancelled,
                    _ => {}
                }
                events.push(Event::Tool(call.clone()));
                if is_new {
                    self.acp.message += 1;
                }
            }
            "usage_update" => {
                usage_events(update, events);
            }
            "current_model_update" => {
                if let Some(model) = update["currentModelId"].as_str() {
                    events.push(Event::Model(model.into()));
                }
            }
            "config_option_update" | "config_options_update" => {
                if update["configOptions"].is_array() {
                    self.acp.setup["configOptions"] = update["configOptions"].clone();
                }
            }
            "plan" => {
                let text = update["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| {
                        e["content"].as_str().map(|text| {
                            format!(
                                "{} {text}",
                                if e["status"] == "completed" {
                                    "✓"
                                } else {
                                    "•"
                                }
                            )
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                events.push(Event::Status(text));
            }
            _ => {}
        }
    }
}

fn config_id(option: &Value) -> &str {
    option["id"]
        .as_str()
        .or_else(|| option["configId"].as_str())
        .unwrap_or("")
}
fn setting_option<'a>(options: &'a [Value], setting: &str) -> Option<&'a Value> {
    let setting = setting.trim().to_lowercase();
    options
        .iter()
        .find(|option| config_id(option).eq_ignore_ascii_case(&setting))
        .or_else(|| {
            options.iter().find(|option| {
                let id = config_id(option);
                match setting.as_str() {
                    "effort" | "reasoning" => {
                        matches!(
                            id,
                            "effort"
                                | "reasoning"
                                | "reasoning_effort"
                                | "thought_level"
                                | "thinking_level"
                        ) || (option["category"] == "thought_level" && id != "thinking")
                    }
                    "fast" | "fastmode" => id.to_lowercase().contains("fast"),
                    "context" | "contextwindow" => matches!(id, "context" | "context_size"),
                    _ => false,
                }
            })
        })
}
fn usage_events(update: &Value, events: &mut Vec<Event>) {
    let usage = update
        .get("usage")
        .or_else(|| update.get("tokenUsage"))
        .or_else(|| update.get("token_usage"))
        .unwrap_or(update);
    let number = |keys: &[&str]| keys.iter().find_map(|key| usage[*key].as_u64());
    let input = number(&["inputTokens", "input_tokens"]);
    let output = number(&["outputTokens", "output_tokens"]);
    let read = number(&["cacheReadTokens", "cache_read_input_tokens"]);
    let write = number(&["cacheWriteTokens", "cache_creation_input_tokens"]);
    let used = number(&["totalTokens", "used", "usedTokens", "used_tokens"])
        .or_else(|| input.zip(output).map(|(i, o)| i + o));
    let window = number(&[
        "window",
        "size",
        "contextWindow",
        "context_window",
        "maxTokens",
        "max_tokens",
    ]);
    if used.is_some() || window.is_some() {
        events.push(Event::Context { used, window });
    }
    if input.is_some() || output.is_some() || read.is_some() || write.is_some() {
        let total = input.unwrap_or(0) + read.unwrap_or(0) + write.unwrap_or(0);
        events.push(Event::Metrics(TurnMetrics {
            input_tokens: input.unwrap_or(0),
            output_tokens: output.unwrap_or(0),
            cache_read_tokens: read.unwrap_or(0),
            cache_write_tokens: write.unwrap_or(0),
            cache_hit_percent: ((read.is_some() || write.is_some()) && total > 0)
                .then(|| read.unwrap_or(0) as f64 / total as f64 * 100.),
        }));
    }
}

pub(super) fn content_text(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.into();
    }
    if let Some(values) = value.as_array() {
        return values
            .iter()
            .map(content_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(text) = value["text"].as_str() {
        return text.into();
    }
    if let Some(content) = value.get("content") {
        return content_text(content);
    }
    if value["type"] == "diff" {
        return format!(
            "{}\n{}",
            value["path"].as_str().unwrap_or(""),
            value["newText"].as_str().unwrap_or("")
        );
    }
    if value.is_null() {
        String::new()
    } else {
        serde_json::to_string_pretty(value).unwrap_or_default()
    }
}

pub(super) fn respond(
    stdin: &mut impl Write,
    wire: Value,
    request: &Value,
    decision: Decision,
) -> Result<(), String> {
    let preference = match &decision {
        Decision::Allow => ["allow_once", "allow_always"],
        Decision::Session => ["allow_always", "allow_once"],
        Decision::Deny | Decision::KeepOpen | Decision::Answer(_) | Decision::Steer(_) => {
            ["reject_once", "reject_always"]
        }
    };
    let options: Vec<_> = request["options"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let option = preference
        .iter()
        .find_map(|kind| options.iter().find(|o| o["kind"] == *kind))
        .or_else(|| {
            preference.iter().find_map(|kind| {
                options
                    .iter()
                    .find(|o| o["optionId"] == *kind || o["optionId"] == kind.replace('_', "-"))
            })
        });
    let outcome = option
        .and_then(|o| o["optionId"].as_str().or_else(|| o["option_id"].as_str()))
        .map(|id| json!({"outcome":"selected","optionId":id}))
        .unwrap_or_else(|| json!({"outcome":"cancelled"}));
    write_json(
        stdin,
        json!({"jsonrpc":"2.0","id":wire,"result":{"outcome":outcome}}),
    )
}

#[cfg(test)]
mod tests;
