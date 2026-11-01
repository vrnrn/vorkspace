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
mod tests {
    use super::*;
    fn protocol(kind: Kind) -> Protocol {
        Protocol::new(Request {
            kind,
            binary: "unused".into(),
            cwd: "/tmp".into(),
            prompt: "Hello".into(),
            provider_id: None,
            model: None,
            settings: Default::default(),
        })
    }
    fn last(out: &mut Vec<u8>) -> Value {
        let text = String::from_utf8(std::mem::take(out)).unwrap();
        serde_json::from_str(text.lines().last().unwrap()).unwrap()
    }
    #[test]
    fn antigravity_runs_its_native_session_lifecycle() {
        for kind in Kind::ALL.into_iter().filter(|k| k.is_acp()) {
            let mut p = protocol(kind);
            let mut out = Vec::new();
            p.acp_session(&mut out).unwrap();
            let new = last(&mut out);
            assert_eq!(new["method"], "session/new");
            let events = p.frame(json!({"id":new["id"],"result":{"sessionId":"s","models":{"currentModelId":"m"}}}),&mut out).unwrap();
            assert!(events.contains(&Event::SessionId("s".into())));
            loop {
                let request = last(&mut out);
                if request["method"] == "session/prompt" {
                    assert_eq!(request["params"]["prompt"][0]["text"], "Hello");
                    let live = p.frame(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello"}}}}),&mut out).unwrap();
                    assert!(live.contains(&Event::Text("Hello".into())));
                    assert_eq!(
                        p.frame(
                            json!({"id":request["id"],"result":{"stopReason":"end_turn"}}),
                            &mut out
                        )
                        .unwrap(),
                        vec![Event::Done]
                    );
                    break;
                }
                p.frame(json!({"id":request["id"],"result":{}}), &mut out)
                    .unwrap();
            }
            assert!(p.done);
        }
    }
    #[test]
    fn resuming_mutes_replay_and_replaces_missing_sessions_like_the_desktop() {
        let mut p = protocol(Kind::Antigravity);
        p.request.provider_id = Some("saved".into());
        let mut out = Vec::new();
        p.acp_session(&mut out).unwrap();
        let request = last(&mut out);
        assert_eq!(request["method"], "session/resume");
        p.frame(
            json!({"id":request["id"],"error":{"code":-32601,"message":"unsupported"}}),
            &mut out,
        )
        .unwrap();
        let request = last(&mut out);
        assert_eq!(request["method"], "session/load");
        assert!(p.frame(json!({"method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"text":"Old history"}}}}),&mut out).unwrap().is_empty());
        p.frame(
            json!({"id":request["id"],"error":{"message":"Session missing"}}),
            &mut out,
        )
        .unwrap();
        let replacement = last(&mut out);
        assert_eq!(replacement["method"], "session/new");
        let events = p
            .frame(
                json!({"id":replacement["id"],"result":{"sessionId":"replacement"}}),
                &mut out,
            )
            .unwrap();
        assert!(events.contains(&Event::SessionId("replacement".into())));
        assert_eq!(p.acp.session, "replacement");
        let mode = last(&mut out);
        assert_eq!(mode["method"], "session/set_mode");
        assert_eq!(mode["params"]["modeId"], "default");
        p.frame(json!({"id":mode["id"],"result":{}}), &mut out)
            .unwrap();
        assert_eq!(last(&mut out)["method"], "session/prompt");
    }
    #[test]
    fn uncertain_resume_or_load_never_starts_a_replacement_session() {
        for load in [false, true] {
            let mut protocol = protocol(Kind::Antigravity);
            protocol.request.provider_id = Some("saved".into());
            let mut out = Vec::new();
            protocol.acp_session(&mut out).unwrap();
            let resume = last(&mut out);
            if load {
                protocol
                    .frame(
                        json!({"id":resume["id"],"error":{"message":"Unsupported method"}}),
                        &mut out,
                    )
                    .unwrap();
                assert_eq!(last(&mut out)["method"], "session/load");
            }
            assert!(protocol
                .acp_failure(&json!({"message":"Request timed out"}), true, &mut out)
                .unwrap_err()
                .contains("Request timed out"));
            assert!(
                out.is_empty(),
                "an uncertain external result cannot authorize a new session"
            );
            assert_eq!(protocol.request.provider_id.as_deref(), Some("saved"));
        }
    }
    #[test]
    fn permissions_use_semantic_kinds_and_never_guess_opaque_allow_options() {
        let params = json!({"options":[{"optionId":"opaque-deny","kind":"reject_once"},{"optionId":"opaque-allow","kind":"allow_once"},{"optionId":"opaque-session","kind":"allow_always"}]});
        for (decision, expected) in [
            (Decision::Allow, "opaque-allow"),
            (Decision::Session, "opaque-session"),
            (Decision::Deny, "opaque-deny"),
        ] {
            let mut out = Vec::new();
            respond(&mut out, json!("permission"), &params, decision).unwrap();
            assert_eq!(last(&mut out)["result"]["outcome"]["optionId"], expected);
        }
        let mut out = Vec::new();
        respond(
            &mut out,
            json!(1),
            &json!({"options":[{"optionId":"unknown"}]}),
            Decision::Allow,
        )
        .unwrap();
        assert_eq!(last(&mut out)["result"]["outcome"]["outcome"], "cancelled");
    }
    #[test]
    fn tool_updates_preserve_identity_input_and_partial_results() {
        let mut p = protocol(Kind::Antigravity);
        p.acp.stage = Stage::Prompt;
        p.acp.session = "s".into();
        for update in [
            json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"Read","kind":"read","rawInput":{"path":"file.rs"}}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"t","content":[{"type":"content","content":{"type":"text","text":"contents"}}]}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"completed"}),
        ] {
            p.frame(
                json!({"method":"session/update","params":{"sessionId":"s","update":update}}),
                &mut Vec::new(),
            )
            .unwrap();
        }
        let tool = &p.tools["t"];
        assert_eq!(tool.name, "Read");
        assert!(tool.input.contains("file.rs"));
        assert_eq!(tool.output, "contents");
        assert_eq!(tool.state, ToolState::Completed);
    }
    #[test]
    fn live_acp_tool_keeps_semantic_kind_locations_and_original_input() {
        let mut protocol = protocol(Kind::Antigravity);
        let raw = json!({"command":"/bin/zsh -lc 'cat src/main.rs'","future":true});
        protocol.acp_update(&json!({"sessionUpdate":"tool_call","toolCallId":"shell","kind":"execute","title":"Inspect project","rawInput":raw}), &mut Vec::new());
        let call = &protocol.tools["shell"];
        assert_eq!(call.summary(), ("Read".into(), "src/main.rs".into()));
        assert_eq!(
            serde_json::from_str::<Value>(&call.input_detail()).unwrap(),
            raw
        );
        protocol.acp_update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"shell","status":"completed","content":{"text":"contents"}}), &mut Vec::new());
        let call = &protocol.tools["shell"];
        assert_eq!(call.summary().0, "Read");
        assert_eq!(call.output, "contents");
        assert_eq!(
            serde_json::from_str::<Value>(&call.input_detail()).unwrap(),
            raw
        );
        protocol.acp_update(&json!({"sessionUpdate":"tool_call","toolCallId":"file","kind":"read","title":"Inspect file","locations":[{"path":"src/lib.rs","line":18}]}), &mut Vec::new());
        let shown = protocol.tools["file"].presentation();
        assert_eq!(
            (shown.action.as_str(), shown.target.as_str()),
            ("Read", "src/lib.rs")
        );
        assert_eq!(shown.start_line, Some(18));
    }
    #[test]
    fn planning_denies_execution_even_in_full_access() {
        let mut p = protocol(Kind::Antigravity);
        p.acp.stage = Stage::Prompt;
        p.request.settings.plan = true;
        p.request.settings.access = AccessMode::FullAccess;
        let mut out = Vec::new();
        let events = p.frame(json!({"id":"permission","method":"session/request_permission","params":{"toolCall":{"kind":"execute"},"options":[{"kind":"allow_once","optionId":"yes"},{"kind":"reject_once","optionId":"no"}]}}),&mut out).unwrap();
        assert!(events.is_empty());
        assert_eq!(last(&mut out)["result"]["outcome"]["optionId"], "no");
    }
    #[test]
    fn sparse_permission_requests_keep_the_tool_preview() {
        let mut p = protocol(Kind::Antigravity);
        p.acp.stage = Stage::Prompt;
        let mut tool = ToolCall::new("t", "Write example.rs");
        tool.input = "{\"path\":\"example.rs\"}".into();
        tool.output = "+ new content".into();
        p.tools.insert("t".into(), tool);
        p.acp.tool_kinds.insert("t".into(), "edit".into());
        let events = p.frame(json!({"id":99,"method":"session/request_permission","params":{"toolCall":{"toolCallId":"t"},"options":[{"kind":"allow_once","optionId":"yes"},{"kind":"reject_once","optionId":"no"}]}}),&mut Vec::new()).unwrap();
        let Event::Approval(request) = &events[0] else {
            panic!("expected approval")
        };
        assert_eq!(request.title, "Allow file changes?");
        assert!(request.detail.contains("example.rs") && request.detail.contains("+ new content"));
    }
    #[test]
    fn usage_preserves_context_and_cache_metrics() {
        let mut events = Vec::new();
        usage_events(
            &json!({"usage":{"inputTokens":100,"outputTokens":50,"cacheReadTokens":900,"totalTokens":150,"contextWindow":200000}}),
            &mut events,
        );
        assert!(events.contains(&Event::Context {
            used: Some(150),
            window: Some(200000)
        }));
        assert!(events.iter().any(|event| matches!(event, Event::Metrics(metrics) if metrics.input_tokens == 100 && metrics.output_tokens == 50 && metrics.cache_read_tokens == 900 && metrics.cache_hit_percent == Some(90.))));
    }
    #[test]
    fn controls_use_the_options_advertised_after_model_selection() {
        let mut p = protocol(Kind::Antigravity);
        p.request.model = Some("chosen".into());
        p.request.settings.effort = Some("high".into());
        p.request
            .settings
            .model_settings
            .insert("context".into(), "1m".into());
        p.request
            .settings
            .model_settings
            .insert("unsupported".into(), "ignored".into());
        let mut out = Vec::new();
        p.acp_session(&mut out).unwrap();
        let new = last(&mut out);
        p.frame(json!({"id":new["id"],"result":{"sessionId":"s","configOptions":[{"id":"model","category":"model"}]}}),&mut out).unwrap();
        let select = last(&mut out);
        assert_eq!(select["params"]["value"], "chosen");
        p.frame(json!({"id":select["id"],"result":{"configOptions":[{"id":"effort","category":"thought_level"},{"id":"fast","type":"boolean"},{"id":"context"}]}}),&mut out).unwrap();
        let mut choices = Vec::new();
        loop {
            let request = last(&mut out);
            if request["method"] == "session/prompt" {
                break;
            }
            choices.push(request["params"].clone());
            p.frame(json!({"id":request["id"],"result":{}}), &mut out)
                .unwrap();
        }
        assert_eq!(choices.len(), 4);
        assert!(choices.iter().any(|params| params["modeId"] == "default"));
        assert!(choices
            .iter()
            .any(|p| p["configId"] == "context" && p["value"] == "1m"));
        assert!(choices
            .iter()
            .any(|p| p["configId"] == "effort" && p["value"] == "high"));
        assert!(choices
            .iter()
            .any(|p| p["configId"] == "fast" && p["type"] == "boolean" && p["value"] == false));
    }
    #[test]
    fn antigravity_auto_edit_only_approves_edits_and_read_only_denies_execution() {
        for (access, tool, expected_approval) in [
            (AccessMode::AutoAcceptEdits, "edit", true),
            (AccessMode::AutoAcceptEdits, "execute", false),
            (AccessMode::ReadOnly, "execute", false),
        ] {
            let mut protocol = protocol(Kind::Antigravity);
            protocol.request.settings.access = access;
            protocol.acp.stage = Stage::Prompt;
            let mut out = Vec::new();
            let events = protocol.frame(json!({"id":"permission","method":"session/request_permission","params":{"toolCall":{"kind":tool},"options":[{"kind":"allow_once","optionId":"yes"},{"kind":"reject_once","optionId":"no"}]}}), &mut out).unwrap();
            if access == AccessMode::ReadOnly || expected_approval {
                assert!(events.is_empty());
                assert_eq!(
                    last(&mut out)["result"]["outcome"]["optionId"],
                    if expected_approval { "yes" } else { "no" }
                );
            } else {
                assert!(matches!(events.as_slice(), [Event::Approval(_)]));
                assert!(out.is_empty());
            }
        }
    }
}
