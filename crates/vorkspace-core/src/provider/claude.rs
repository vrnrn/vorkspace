use super::*;

impl Protocol {
    pub(super) fn claude_message_start(&mut self, v: &Value) {
        let starts_message = v["parent_tool_use_id"].is_null()
            && (v["type"] == "assistant"
                || (v["type"] == "stream_event" && v["event"]["type"] == "message_start")
                || (v["type"] == "system" && v["subtype"] == "init"));
        if starts_message && self.claude_result_seen {
            self.claude_result_seen = false;
            self.claude_resume_at = None;
            self.text_key = None;
        }
    }

    pub(super) fn claude_cancel(&mut self, v: &Value) -> Vec<Event> {
        let id = v["request_id"].to_string();
        let mut events = Vec::new();
        if self.approvals.remove(&id).is_some() {
            events.push(Event::ApprovalResolved(id.clone()));
        }
        if self.questions.remove(&id).is_some() {
            if self.visible_question.as_deref() == Some(id.as_str()) {
                self.visible_question = None;
            }
            events.push(Event::QuestionResolved(id));
            events.extend(self.show_next_question());
        }
        events
    }

    pub(super) fn claude_frame(
        &mut self,
        v: Value,
        mut events: Vec<Event>,
        stdin: &mut impl Write,
    ) -> Result<DecodedFrame, String> {
        if let Some(id) = v["session_id"].as_str() {
            events.push(Event::SessionId(id.into()));
        }
        if let Some(model) = v["model"].as_str().filter(|_| v["type"] == "system") {
            events.push(Event::Model(model.into()));
        }
        match v["type"].as_str().unwrap_or("") {
            "rate_limit_event" if v["parent_tool_use_id"].is_null() => {
                self.claude_usage_limit =
                    crate::limit::Limit::from_claude_info(&v["rate_limit_info"]);
            }
            "control_response" if v["response"]["request_id"] == "native-init" => {
                self.claude_prompt(stdin)?;
            }
            "stream_event" => {
                self.claude_stream(&v, &mut events);
            }
            "assistant" => {
                if let Some(error) = v["error"].as_str() {
                    return Err(self.claude_error(error, &v));
                }
                if v["parent_tool_use_id"].is_null() {
                    if let Some(used) = crate::context::claude_used(&v["message"]["usage"]) {
                        events.push(Event::Context {
                            used: Some(used),
                            window: None,
                        });
                    }
                }
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_use" {
                        let id = block["id"].as_str().unwrap_or("");
                        let tool = self.tools.entry(id.into()).or_insert_with(|| {
                            ToolCall::new(id, block["name"].as_str().unwrap_or("Tool"))
                        });
                        tool.input =
                            serde_json::to_string_pretty(&block["input"]).unwrap_or_default();
                        tool.refresh_agent_input();
                        events.push(Event::Tool(tool.clone()));
                    }
                }
                let text = v["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<String>();
                self.text(&self.claude_message.to_string(), &text, true, &mut events);
            }
            "user" => {
                self.claude_user(&v, &mut events);
            }
            "result" => {
                self.claude_result(&v, &mut events)?;
            }
            "control_request" => {
                let request = &v["request"];
                if request["subtype"] == "can_use_tool" {
                    if request["tool_name"] == "AskUserQuestion" {
                        self.question(
                            v["request_id"].clone(),
                            "claude",
                            request.clone(),
                            &mut events,
                            stdin,
                        )?;
                        return Ok(events.into());
                    }
                    let detail =
                        serde_json::to_string_pretty(&request["input"]).unwrap_or_default();
                    self.approval(
                        v["request_id"].clone(),
                        request.clone(),
                        ApprovalDetails {
                            kind: "claude",
                            detail,
                        },
                        stdin,
                        &mut events,
                    )?;
                } else {
                    write_json(
                        stdin,
                        json!({"type":"control_response","response":{"subtype":"error","request_id":v["request_id"],"error":"Unsupported host request"}}),
                    )?;
                }
            }
            _ => {}
        }

        Ok(events.into())
    }

    fn claude_stream(&mut self, v: &Value, events: &mut Vec<Event>) {
        let e = &v["event"];
        if e["type"] == "message_start" {
            self.claude_message += 1;
            self.claude_tools.clear();
        }
        if e["type"] == "content_block_start" && e["content_block"]["type"] == "tool_use" {
            let block = &e["content_block"];
            let id = block["id"].as_str().unwrap_or("");
            let tool = ToolCall::new(id, block["name"].as_str().unwrap_or("Tool"));
            self.claude_tools
                .insert(e["index"].as_u64().unwrap_or(0), (id.into(), String::new()));
            self.tools.insert(id.into(), tool.clone());
            events.push(Event::Tool(tool));
        }
        if e["type"] == "content_block_delta" && e["delta"]["type"] == "input_json_delta" {
            // Keep the last input that
            // parses; a half-streamed object is not shown.
            if let Some((id, partial)) =
                self.claude_tools.get_mut(&e["index"].as_u64().unwrap_or(0))
            {
                partial.push_str(e["delta"]["partial_json"].as_str().unwrap_or(""));
                let parsed =
                    serde_json::from_str::<Value>(partial).is_ok_and(|value| value.is_object());
                if let Some(tool) = self.tools.get_mut(id.as_str()).filter(|_| parsed) {
                    tool.input = partial.clone();
                    events.push(Event::Tool(tool.clone()));
                }
            }
        }
        if e["type"] == "content_block_delta" && e["delta"]["type"] == "thinking_delta" {
            events.push(Event::Reasoning(
                e["delta"]["thinking"].as_str().unwrap_or("").into(),
            ));
        }
        if e["type"] == "content_block_delta" && e["delta"]["type"] == "text_delta" {
            self.text(
                &self.claude_message.to_string(),
                e["delta"]["text"].as_str().unwrap_or(""),
                false,
                events,
            );
        }
    }

    fn claude_user(&mut self, v: &Value, events: &mut Vec<Event>) {
        for block in v["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_result" {
                // A background Agent's launch receipt is not its final report.
                if block["is_error"] != true
                    && block["tool_use_id"]
                        .as_str()
                        .is_some_and(|id| self.activities.background_calls.contains(id))
                {
                    continue;
                }
                if let Some(tool) = block["tool_use_id"]
                    .as_str()
                    .and_then(|id| self.tools.get_mut(id))
                {
                    tool.output =
                        block["content"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| {
                                block["content"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|b| b["text"].as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            });
                    tool.bound_output();
                    tool.state = if block["is_error"] == true {
                        ToolState::Failed
                    } else {
                        ToolState::Completed
                    };
                    events.push(Event::Tool(tool.clone()));
                }
            }
        }
    }

    fn claude_result(&mut self, v: &Value, events: &mut Vec<Event>) -> Result<(), String> {
        if self.compact && !self.compact_confirmed && v["is_error"] != true {
            return Err("Claude Code did not confirm context compaction".into());
        }
        let usage = &v["usage"];
        let input = usage["input_tokens"].as_u64().unwrap_or(0);
        let output = usage["output_tokens"].as_u64().unwrap_or(0);
        let read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
        let write = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        let metrics = TurnMetrics {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: read,
            cache_write_tokens: write,
            cache_hit_percent: ((usage.get("cache_read_input_tokens").is_some()
                || usage.get("cache_creation_input_tokens").is_some())
                && input + read + write > 0)
                .then(|| read as f64 / (input + read + write) as f64 * 100.),
        };
        if metrics.reported() {
            events.push(Event::Metrics(metrics));
        }
        let last = v["usage"]["iterations"]
            .as_array()
            .and_then(|iterations| iterations.last())
            .unwrap_or(&v["usage"]);
        let used = crate::context::claude_used(last);
        let window = v["modelUsage"]
            .as_object()
            .into_iter()
            .flat_map(|models| models.values())
            .filter_map(|model| model["contextWindow"].as_u64())
            .filter(|w| *w > 0)
            .max();
        if used.is_some() || window.is_some() {
            events.push(Event::Context { used, window });
        }
        if v["is_error"] == true {
            let error = v["result"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v["errors"].to_string());
            return Err(self.claude_error(&error, v));
        }
        // Some versions omit partial/assistant frames in simple turns.
        if !self.begun {
            self.text(
                &self.claude_message.to_string(),
                v["result"].as_str().unwrap_or(""),
                true,
                events,
            );
        }
        self.claude_result_seen = true;
        if self.activities.claude_background.is_empty()
            && self.activities.claude_pending.is_empty()
            && self.claude_resume_at.is_none()
        {
            events.push(Event::Done);
            self.done = true;
        } else {
            events.push(Event::Status("Waiting for background tasks…".into()));
        }

        Ok(())
    }
}
