use super::*;

impl Protocol {
    pub(super) fn codex_intercept(
        &mut self,
        v: &Value,
        stdin: &mut impl Write,
    ) -> Result<Option<DecodedFrame>, String> {
        // Codex requests its clock before thread/start may have replied.
        if v["method"] == "currentTime/read" && v.get("id").is_some() {
            write_json(
                stdin,
                json!({"id":v["id"],"result":{"currentTimeAt":crate::time::now_ms()/1000}}),
            )?;
            return Ok(Some(Vec::new().into()));
        }
        let turn = v["params"]["turnId"]
            .as_str()
            .or_else(|| v["params"]["turn"]["id"].as_str());
        if turn.is_some() && turn == self.previous_turn.as_deref() {
            return Ok(Some(Vec::new().into()));
        }
        if let Some(id) = v["id"].as_str() {
            if let Some((wire, kind, request, question)) = self.async_answers.remove(id) {
                if let Some(error) = v.get("error").filter(|e| !e.is_null()) {
                    let message = codex_error(error, "Could not send your answer to Codex");
                    self.questions
                        .insert(question.id.clone(), (wire, kind, request, question.clone()));
                    self.visible_question = Some(question.id.clone());
                    return Ok(Some(
                        vec![Event::Status(message), Event::Question(question)].into(),
                    ));
                }
                return Ok(Some(vec![Event::QuestionResolved(question.id)].into()));
            }
            if id.starts_with("native-answer-") || id.starts_with("native-interrupt-") {
                return Ok(Some(Vec::new().into()));
            }
        }
        let method = v["method"].as_str().unwrap_or("");
        if self.previous_turn.is_some()
            && self.active_turn.is_none()
            && method != "turn/started"
            && (method.starts_with("item/") || method.starts_with("turn/") || method == "error")
        {
            return Ok(Some(Vec::new().into()));
        }

        Ok(None)
    }

    pub(super) fn codex_frame(
        &mut self,
        v: Value,
        events: Vec<Event>,
        stdin: &mut impl Write,
    ) -> Result<DecodedFrame, String> {
        if let Some(error) = v.get("error").filter(|e| !e.is_null()) {
            let message = codex_error(error, "RPC request failed");
            if v["id"] == 2
                && self.request.provider_id.is_some()
                && lifecycle::recoverable_codex_resume(&message)
            {
                if self.compact {
                    return Err(
                        "The saved Codex context could not be resumed for compaction.".into(),
                    );
                }
                if self.request.settings.bot.is_some() {
                    return Err("The saved Bot Codex context could not be resumed. Retry to keep its saved context.".into());
                }
                self.request.provider_id = None;
                return self.decode_frame(json!({"id":1,"result":{}}), stdin);
            }
            return Err(message);
        }
        match v["id"].as_u64() {
            Some(1) if v.get("result").is_some() => {
                if !self.initialized {
                    write_json(stdin, json!({"method":"initialized"}))?;
                    self.initialized = true;
                }
                let (policy, sandbox, sandbox_policy, reviewer) = self.codex_access();
                let mut params = json!({"cwd":self.request.cwd,"approvalPolicy":policy,"sandbox":sandbox,"sandboxPolicy":sandbox_policy,"approvalsReviewer":reviewer,
                            "developerInstructions":format!("You are the coding agent in vorkspace. Use the available tools to inspect and work in the supplied cwd. Do not ask the user to paste files or git output you can inspect yourself. Respect the selected access policy and request approval when required. {}", if self.request.settings.plan { "This is a planning turn: inspect and propose a plan without changing files." } else { "" })});
                if self.request.settings.text_only {
                    params["ephemeral"] = true.into();
                    params["developerInstructions"] = "Answer the supplied text request directly. Do not call tools or inspect the workspace.".into();
                }
                if let Some(model) = &self.request.model {
                    params["model"] = model.clone().into();
                }
                if let Some(tier) = self
                    .request
                    .settings
                    .service_tier
                    .as_deref()
                    .filter(|tier| *tier != "default")
                {
                    params["serviceTier"] = tier.into();
                }
                let method = if let Some(id) = &self.request.provider_id {
                    params["threadId"] = id.clone().into();
                    // The app already owns its transcript. Resume the
                    // provider context without serializing its history.
                    params["excludeTurns"] = true.into();
                    "thread/resume"
                } else {
                    "thread/start"
                };
                write_json(stdin, json!({"id":2,"method":method,"params":params}))?;
            }
            Some(2) if v.get("result").is_some() => {
                return self.codex_start_turn(v, events, stdin);
            }
            _ => {}
        }
        self.codex_message(v, events, stdin)
    }

    fn codex_start_turn(
        &mut self,
        v: Value,
        mut events: Vec<Event>,
        stdin: &mut impl Write,
    ) -> Result<DecodedFrame, String> {
        let id = v["result"]["thread"]["id"]
            .as_str()
            .ok_or("Codex returned no thread id")?;
        events.push(Event::SessionId(id.into()));
        if let Some(model) = v["result"]["model"].as_str() {
            events.push(Event::Model(model.into()));
        }
        if self.compact {
            write_json(
                stdin,
                json!({"id":3,"method":"thread/compact/start","params":{"threadId":id}}),
            )?;
            return Ok(events.into());
        }
        let (policy, _, sandbox, reviewer) = self.codex_access();
        let mut params = json!({"threadId":id,"input":self.request.codex_input(),
                            "approvalPolicy":policy,"sandboxPolicy":sandbox,"approvalsReviewer":reviewer});
        if let Some(effort) = &self.request.settings.effort {
            params["effort"] = effort.clone().into();
        }
        if self.request.settings.fast {
            params["serviceTier"] = "fast".into();
        }
        if let Some(tier) = self
            .request
            .settings
            .service_tier
            .as_deref()
            .filter(|tier| *tier != "default")
        {
            params["serviceTier"] = tier.into();
        }
        if self.request.settings.plan {
            params["sandboxPolicy"] = json!({"type":"readOnly"});
            if self.request.settings.operator || self.request.settings.orchestrator {
                params["sandboxPolicy"]["networkAccess"] = true.into();
            }
            params["approvalPolicy"] = "never".into();
        }
        if let Some(model) = self
            .request
            .model
            .as_deref()
            .or_else(|| v["result"]["model"].as_str())
        {
            params["collaborationMode"] = json!({"mode": if self.request.settings.plan { "plan" } else { "default" }, "settings":{"model":model,"reasoning_effort":self.request.settings.effort,"developer_instructions":null}});
        }
        if let Some(model) = &self.request.model {
            params["model"] = model.clone().into();
        }
        write_json(stdin, json!({"id":3,"method":"turn/start","params":params}))?;

        self.codex_message(v, events, stdin)
    }

    fn codex_message(
        &mut self,
        v: Value,
        mut events: Vec<Event>,
        stdin: &mut impl Write,
    ) -> Result<DecodedFrame, String> {
        let p = &v["params"];
        match v["method"].as_str().unwrap_or("") {
            "thread/tokenUsage/updated" => {
                self.codex_usage(p, &mut events);
            }
            "item/agentMessage/delta" => {
                self.text(
                    p["itemId"].as_str().unwrap_or("assistant"),
                    p["delta"].as_str().unwrap_or(""),
                    false,
                    &mut events,
                );
            }
            "item/completed" if p["item"]["type"] == "agentMessage" => {
                self.text(
                    p["item"]["id"].as_str().unwrap_or("assistant"),
                    p["item"]["text"].as_str().unwrap_or(""),
                    true,
                    &mut events,
                );
            }
            "item/started" if p["item"]["type"] == "agentMessage" => {}
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                events.push(Event::Reasoning(p["delta"].as_str().unwrap_or("").into()));
                events.push(Event::Status("Thinking…".into()));
            }
            "item/started" | "item/completed" => {
                self.codex_tool(&p["item"], v["method"] == "item/completed", &mut events)
            }
            "thread/compacted" if self.active_compaction.is_some() => {
                self.compaction(None, ToolState::Completed, &mut events);
            }
            "item/commandExecution/outputDelta" => {
                return Ok(self.codex_output(p));
            }
            "turn/completed" | "turn/aborted" => {
                self.codex_completed(&v, p, &mut events)?;
            }
            "error" if p["willRetry"] != true => {
                return Err(codex_error(&p["error"], "Codex stream error"));
            }
            "error" => {
                events.push(Event::Status(codex_error(
                    &p["error"],
                    "Codex is reconnecting…",
                )));
            }
            "mcpServer/elicitation/request" => {
                self.codex_elicitation(v["id"].clone(), p.clone(), &mut events, stdin)?
            }
            "item/tool/requestUserInput" => {
                self.question(v["id"].clone(), "codex", p.clone(), &mut events, stdin)?
            }
            method
                if v.get("id").is_some()
                    && (method.ends_with("/requestApproval")
                        || matches!(method, "execCommandApproval" | "applyPatchApproval")) =>
            {
                let detail = p["command"]
                    .as_str()
                    .or(p["reason"].as_str())
                    .map(str::to_owned)
                    .unwrap_or_default();
                self.approval(
                    v["id"].clone(),
                    p.clone(),
                    ApprovalDetails {
                        kind: method,
                        detail,
                    },
                    stdin,
                    &mut events,
                )?;
            }
            method if v.get("id").is_some() && !method.is_empty() => {
                // No approval UX in this gate: resolve every server request promptly.
                write_json(
                    stdin,
                    json!({"id":v["id"],"error":{"code":-32601,"message":format!("Unsupported method: {method}")}}),
                )?;
                events.push(Event::Status(format!(
                    "Unsupported Codex request: {method}"
                )));
            }
            _ => {}
        }
        Ok(events.into())
    }

    fn codex_usage(&mut self, p: &Value, events: &mut Vec<Event>) {
        let usage = &p["tokenUsage"];
        events.push(Event::Context {
            used: usage["last"]["totalTokens"].as_u64(),
            window: usage["modelContextWindow"].as_u64().filter(|w| *w > 0),
        });
        let total = usage["total"]["totalTokens"].as_u64().unwrap_or(0);
        if usage["last"].is_object() && (total == 0 || total != self.usage_total) {
            self.usage_total = total;
            let last = &usage["last"];
            self.metrics.input_tokens += last["inputTokens"].as_u64().unwrap_or(0);
            self.metrics.output_tokens += last["outputTokens"].as_u64().unwrap_or(0);
            self.metrics.cache_read_tokens += last["cachedInputTokens"].as_u64().unwrap_or(0);
            self.metrics.cache_write_tokens += last["cacheWriteInputTokens"].as_u64().unwrap_or(0);
            if (last.get("cachedInputTokens").is_some()
                || last.get("cacheWriteInputTokens").is_some())
                && self.metrics.input_tokens > 0
            {
                self.metrics.cache_hit_percent = Some(
                    self.metrics.cache_read_tokens as f64
                        / (self.metrics.input_tokens + self.metrics.cache_write_tokens) as f64
                        * 100.,
                );
            }
            if self.metrics.reported() {
                events.push(Event::Metrics(self.metrics));
            }
        }
    }

    fn codex_output(&mut self, p: &Value) -> DecodedFrame {
        let id = p["itemId"].as_str().unwrap_or("");
        let tool = self
            .tools
            .entry(id.into())
            .or_insert_with(|| ToolCall::new(id, "commandExecution"));
        let delta = p["delta"].as_str().unwrap_or("");
        self.tool_output
            .entry(id.into())
            .or_insert_with(|| output::OutputTail::from_output(std::mem::take(&mut tool.output)))
            .append(delta);
        DecodedFrame::ToolOutput {
            id: id.into(),
            bytes: delta.len(),
        }
    }

    fn codex_completed(
        &mut self,
        v: &Value,
        p: &Value,
        events: &mut Vec<Event>,
    ) -> Result<(), String> {
        let status = p["turn"]["status"].as_str().unwrap_or("completed");
        if status == "failed" {
            return Err(codex_error(&p["turn"]["error"], "Codex turn failed"));
        }
        let interrupted = matches!(status, "interrupted" | "cancelled" | "canceled")
            || v["method"] == "turn/aborted";
        for item in p["turn"]["items"].as_array().into_iter().flatten() {
            if interrupted && item["type"] == "contextCompaction" {
                // An aborted turn can include its unfinished
                // compaction item in the final snapshot.
                continue;
            }
            self.codex_tool(item, true, events);
        }
        if interrupted {
            for (id, output) in self.tool_output.drain() {
                if let Some(tool) = self.tools.get_mut(&id) {
                    tool.output = output.snapshot();
                }
            }
            for tool in self
                .tools
                .values_mut()
                .filter(|tool| tool.state == ToolState::Running)
            {
                tool.state = ToolState::Cancelled;
                events.push(Event::Tool(tool.clone()));
            }
        }
        events.push(Event::Done);
        self.done = true;

        Ok(())
    }

    pub(super) fn codex_tool(&mut self, item: &Value, completed: bool, events: &mut Vec<Event>) {
        let name = item["type"].as_str().unwrap_or("");
        if name == "contextCompaction" {
            self.compaction(
                item["id"].as_str(),
                if completed {
                    ToolState::Completed
                } else {
                    ToolState::Running
                },
                events,
            );
            return;
        }
        if !matches!(
            name,
            "commandExecution" | "fileChange" | "mcpToolCall" | "webSearch" | "dynamicToolCall"
        ) {
            return;
        }
        let id = item["id"].as_str().unwrap_or("");
        let tool = self
            .tools
            .entry(id.into())
            .or_insert_with(|| ToolCall::new(id, name));
        tool.name = item["tool"].as_str().unwrap_or(name).to_owned();
        if name == "mcpToolCall" {
            tool.name = format!("{}:{}", item["server"].as_str().unwrap_or("mcp"), tool.name);
        }
        if let Some(output) = self.tool_output.remove(id) {
            tool.output = output.snapshot();
        }
        Self::codex_tool_input(tool, item, name);
        if let Some(output) = item["aggregatedOutput"]
            .as_str()
            .or_else(|| item["output"].as_str())
        {
            tool.output = output.to_owned();
        } else if let Some(result) = item.get("result") {
            tool.output = serde_json::to_string_pretty(result).unwrap_or_default();
        }
        tool.bound_output();
        tool.state = match item["status"].as_str().unwrap_or(if completed {
            "completed"
        } else {
            "inProgress"
        }) {
            "completed" => ToolState::Completed,
            "failed" | "declined" => ToolState::Failed,
            "cancelled" | "canceled" | "interrupted" | "aborted" => ToolState::Cancelled,
            _ => ToolState::Running,
        };
        if item["exitCode"].as_i64().is_some_and(|code| code != 0) {
            tool.state = ToolState::Failed;
        }
        events.push(Event::Tool(tool.clone()));
    }

    fn codex_tool_input(tool: &mut ToolCall, item: &Value, name: &str) {
        let previous_input = tool.input.clone();
        tool.input = item["command"]
            .as_str()
            .or_else(|| item["query"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                serde_json::to_string_pretty(
                    item.get("arguments")
                        .or_else(|| item.get("changes"))
                        .unwrap_or(&Value::Null),
                )
                .unwrap_or_default()
            });
        if name == "commandExecution" {
            // Keep argv and parsed provider metadata intact. Presentation unwraps
            // recognized launchers without changing the recorded execution.
            let previous = serde_json::from_str::<Value>(&previous_input).ok();
            let mut input = previous
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            for field in [
                "command",
                "commandActions",
                "parsed_cmd",
                "parsedCmd",
                "cwd",
                "exitCode",
                "durationMs",
            ] {
                if let Some(value) = item.get(field) {
                    input[field] = value.clone();
                }
            }
            tool.input = input.to_string();
        }
        if let Some(changes) = item["changes"].as_array().filter(|_| name == "fileChange") {
            let mut input = json!({"path": changes.first().and_then(|c| c["path"].as_str()).unwrap_or("Files")});
            if changes.len() > 1 {
                // The caption names the first file; this keeps the rest.
                input["paths"] = changes.iter().filter_map(|c| c["path"].as_str()).collect();
            }
            tool.input = input.to_string();
            tool.output = changes
                .iter()
                .map(|c| {
                    format!(
                        "{}\n{}",
                        c["path"].as_str().unwrap_or(""),
                        c["diff"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
}
