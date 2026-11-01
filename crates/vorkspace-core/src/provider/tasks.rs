//! Keep delegated traffic under its real parent, rather than merging it into
//! the assistant's answer or allowing a child completion to finish the turn.
use super::*;
use crate::tool::tasks::{AgentRun, AgentStep, Panel, TaskList};

#[derive(Default)]
pub(super) struct Activities {
    main_thread: Option<String>,
    owners: HashMap<String, String>,
    pending: Vec<Value>,
    reported_states: HashMap<String, ToolState>,
    pub(super) background_calls: std::collections::HashSet<String>,
    pub(super) claude_background: HashMap<String, String>,
    pub(super) claude_pending: std::collections::HashSet<String>,
}
fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.trim().is_empty())
}
fn state(status: &str) -> Option<ToolState> {
    match status.to_lowercase().as_str() {
        "completed" | "complete" | "done" | "finished" | "closed" | "stopped" => {
            Some(ToolState::Completed)
        }
        "failed" | "error" | "errored" | "notfound" | "not_found" => Some(ToolState::Failed),
        "interrupted" | "cancelled" | "canceled" | "killed" | "aborted" => {
            Some(ToolState::Cancelled)
        }
        "running" | "inprogress" | "in_progress" | "pending" => Some(ToolState::Running),
        _ => None,
    }
}
impl Protocol {
    pub(super) fn activity(&mut self, value: &Value, events: &mut Vec<Event>) -> bool {
        match self.request.kind {
            Kind::Codex => self.codex_activity(value, events),
            Kind::Claude => self.claude_activity(value, events),
            _ => false,
        }
    }
    fn agent_step(
        &mut self,
        parent: &str,
        step: AgentStep,
        model: Option<&str>,
        events: &mut Vec<Event>,
    ) {
        let Some(tool) = self.tools.get_mut(parent) else {
            return;
        };
        let Some(run) = tool.agent_mut() else { return };
        if let Some(model) = model {
            run.model = Some(model.into());
        }
        run.upsert(step);
        events.push(Event::Tool(tool.clone()));
    }
    fn codex_activity(&mut self, value: &Value, events: &mut Vec<Event>) -> bool {
        if value["id"] == 2 {
            if let Some(id) = text(&value["result"]["thread"]["id"]) {
                self.activities.main_thread = Some(id.into());
            }
        }
        let p = &value["params"];
        let method = value["method"].as_str().unwrap_or("");
        // Approval/question requests still go through the shared approval UI.
        if value.get("id").is_some()
            && (method.ends_with("/requestApproval") || method == "item/tool/requestUserInput")
        {
            return false;
        }
        let thread = text(&p["threadId"])
            .or_else(|| text(&p["thread_id"]))
            .or_else(|| text(&p["thread"]["id"]));
        if let Some(parent) = thread
            .and_then(|id| self.activities.owners.get(id))
            .cloned()
        {
            self.codex_child(&parent, method, p, events);
            return true;
        }
        if let (Some(thread), Some(main)) = (thread, self.activities.main_thread.as_deref()) {
            if thread != main {
                // A child can speak before its spawn snapshot identifies it.
                if matches!(
                    method,
                    "item/started"
                        | "item/completed"
                        | "turn/completed"
                        | "turn/aborted"
                        | "thread/started"
                ) && self.activities.pending.len() < 64
                    && value.to_string().len() <= 16 * 1024
                {
                    self.activities.pending.push(value.clone());
                }
                return true;
            }
        }
        if method == "turn/completed" {
            for item in p["turn"]["items"].as_array().into_iter().flatten() {
                if matches!(
                    item["type"].as_str(),
                    Some("collabAgentToolCall" | "subAgentActivity")
                ) {
                    self.codex_activity(&json!({"method":"item/completed","params":{"threadId":thread,"item":item}}), events);
                }
            }
        }
        if method == "turn/plan/updated" {
            let Some(list) = TaskList::from_value(p) else {
                return true;
            };
            let Some(turn) = text(&p["turnId"]).or(self.active_turn.as_deref()) else {
                return true;
            };
            let id = format!("plan:{turn}");
            let tool = self
                .tools
                .entry(id.clone())
                .or_insert_with(|| ToolCall::new(&id, "update_plan"));
            tool.input = p.to_string();
            tool.panel = Some(Panel::Tasks(list));
            tool.state = ToolState::Completed; // A received snapshot, not a claim its tasks finished.
            events.push(Event::Tool(tool.clone()));
            return true;
        }
        if !matches!(method, "item/started" | "item/completed") {
            return false;
        }
        let item = &p["item"];
        if !matches!(
            item["type"].as_str(),
            Some("collabAgentToolCall" | "subAgentActivity")
        ) {
            return false;
        }
        let Some(id) = text(&item["id"]) else {
            return true;
        };
        let action = item["tool"].as_str().unwrap_or("");
        let mut ids: Vec<String> = ["agentThreadId", "agent_thread_id"]
            .iter()
            .filter_map(|key| text(&item[*key]).map(str::to_owned))
            .collect();
        for receiver in item
            .get("receiverThreadIds")
            .or_else(|| item.get("receiver_thread_ids"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(receiver) = text(receiver) {
                ids.push(receiver.into());
            }
        }
        let states = item
            .get("agentsStates")
            .or_else(|| item.get("agents_states"))
            .and_then(Value::as_object);
        if let Some(states) = states {
            ids.extend(states.keys().cloned());
        }
        ids.sort();
        ids.dedup();
        let spawn =
            matches!(action, "spawnAgent" | "resumeAgent") || item["type"] == "subAgentActivity";
        if spawn {
            let existing = ids
                .iter()
                .find_map(|id| self.activities.owners.get(id))
                .cloned();
            let owner = existing.as_deref().unwrap_or(id);
            let tool = self
                .tools
                .entry(owner.into())
                .or_insert_with(|| ToolCall::new(owner, "collabAgentToolCall"));
            if tool.panel.is_none() {
                tool.panel = Some(Panel::Agent(AgentRun::default()));
            }
            tool.input = item.to_string();
            if let Some(run) = tool.agent_mut() {
                for id in &ids {
                    if !run.ids.contains(id) {
                        run.ids.push(id.clone());
                    }
                    self.activities
                        .owners
                        .entry(id.clone())
                        .or_insert_with(|| owner.into());
                }
                // A second item may describe an already-named child. Keep the
                // original task title, but let it fill a missing path label.
                if owner == id || (run.name.is_none() && run.brief.is_empty()) {
                    if let Some(name) = AgentRun::from_input(item).name {
                        run.name = Some(name);
                    }
                }
                if let Some(model) = text(&item["model"]) {
                    run.model = Some(model.into());
                }
                if let Some(brief) = text(&item["prompt"]).or_else(|| text(&item["description"])) {
                    run.brief = brief.into();
                }
            }
            // The spawn RPC returning does not mean the agent has completed.
            if item["status"] == "failed" || item["kind"] == "interrupted" {
                tool.state = ToolState::Failed;
                if let Some(message) =
                    text(&item["error"]["message"]).or_else(|| text(&item["message"]))
                {
                    tool.output = message.into();
                    tool.bound_output();
                }
            }
            events.push(Event::Tool(tool.clone()));
        }
        if let Some(states) = states {
            for (thread, reported) in states {
                if let Some(status) = reported["status"].as_str().and_then(state) {
                    self.settle_child(thread, status, text(&reported["message"]), events);
                }
            }
        }
        let pending = std::mem::take(&mut self.activities.pending);
        for frame in pending {
            let thread =
                text(&frame["params"]["threadId"]).or_else(|| text(&frame["params"]["thread_id"]));
            if let Some(owner) = thread
                .and_then(|id| self.activities.owners.get(id))
                .cloned()
            {
                self.codex_child(
                    &owner,
                    frame["method"].as_str().unwrap_or(""),
                    &frame["params"],
                    events,
                );
            } else {
                self.activities.pending.push(frame);
            }
        }
        true
    }
    fn codex_child(&mut self, parent: &str, method: &str, params: &Value, events: &mut Vec<Event>) {
        if method == "thread/started" {
            if let Some(model) = text(&params["thread"]["model"]) {
                if let Some(tool) = self.tools.get_mut(parent) {
                    if let Some(run) = tool.agent_mut() {
                        run.model = Some(model.into());
                    }
                    events.push(Event::Tool(tool.clone()));
                }
            }
        } else if matches!(method, "item/started" | "item/completed") {
            let item = &params["item"];
            let Some(id) = text(&item["id"]) else { return };
            let kind = item["type"].as_str().unwrap_or("");
            if matches!(kind, "agentMessage" | "reasoning") {
                if method != "item/completed" {
                    return;
                }
                let narration = text(&item["text"]).map(str::to_owned).unwrap_or_else(|| {
                    item["summary"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|part| text(&part["text"]))
                        .collect::<Vec<_>>()
                        .join("\n")
                });
                if !narration.is_empty() {
                    self.agent_step(
                        parent,
                        AgentStep {
                            id: id.into(),
                            kind: if kind == "reasoning" {
                                "reasoning"
                            } else {
                                "message"
                            }
                            .into(),
                            text: narration,
                            state: None,
                            detail: String::new(),
                            ..AgentStep::default()
                        },
                        None,
                        events,
                    );
                }
            } else {
                let mut child_events = Vec::new();
                self.codex_tool(item, method == "item/completed", &mut child_events);
                for child in child_events {
                    if let Event::Tool(child) = child {
                        self.agent_step(parent, AgentStep::from_tool_call(&child), None, events);
                    }
                }
            }
        } else if matches!(method, "turn/completed" | "turn/aborted") {
            if let (Some(thread), Some(status)) = (
                text(&params["threadId"]).or_else(|| text(&params["thread_id"])),
                params["turn"]["status"]
                    .as_str()
                    .and_then(state)
                    .or_else(|| (method == "turn/aborted").then_some(ToolState::Cancelled)),
            ) {
                self.settle_child(
                    thread,
                    status,
                    text(&params["turn"]["error"]["message"]),
                    events,
                );
            }
        }
    }
    fn settle_child(
        &mut self,
        thread: &str,
        status: ToolState,
        message: Option<&str>,
        events: &mut Vec<Event>,
    ) {
        self.activities
            .reported_states
            .insert(thread.into(), status);
        let Some(owner) = self.activities.owners.get(thread) else {
            return;
        };
        let Some(tool) = self.tools.get_mut(owner) else {
            return;
        };
        if let Some(run) = tool.agent_mut() {
            let states = &self.activities.reported_states;
            tool.state = if run
                .ids
                .iter()
                .any(|id| states.get(id) == Some(&ToolState::Failed))
            {
                ToolState::Failed
            } else if !run.ids.is_empty()
                && run.ids.iter().all(|id| {
                    matches!(
                        states.get(id),
                        Some(ToolState::Completed | ToolState::Cancelled)
                    )
                })
                && run
                    .ids
                    .iter()
                    .any(|id| states.get(id) == Some(&ToolState::Cancelled))
            {
                ToolState::Cancelled
            } else if !run.ids.is_empty()
                && run
                    .ids
                    .iter()
                    .all(|id| states.get(id) == Some(&ToolState::Completed))
            {
                ToolState::Completed
            } else {
                ToolState::Running
            };
        }
        if let Some(message) = message {
            tool.output = message.into();
            tool.bound_output();
        }
        events.push(Event::Tool(tool.clone()));
    }
    fn claude_activity(&mut self, value: &Value, events: &mut Vec<Event>) -> bool {
        let parent = text(&value["parent_tool_use_id"]);
        if let Some(parent) = parent {
            match value["type"].as_str() {
                Some("assistant") => {
                    let model = text(&value["message"]["model"]);
                    let message_id = text(&value["message"]["id"]);
                    for (index, block) in value["message"]["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                    {
                        match block["type"].as_str() {
                            Some("tool_use") => {
                                let Some(id) = text(&block["id"]) else {
                                    continue;
                                };
                                let mut child =
                                    ToolCall::new(id, block["name"].as_str().unwrap_or("Tool"));
                                child.input = block["input"].to_string();
                                self.agent_step(
                                    parent,
                                    AgentStep::from_tool_call(&child),
                                    model,
                                    events,
                                );
                            }
                            Some("text" | "thinking") => {
                                let Some(message_id) = message_id else {
                                    continue;
                                };
                                let thinking = block["type"] == "thinking";
                                let narration = block[if thinking { "thinking" } else { "text" }]
                                    .as_str()
                                    .unwrap_or("");
                                if narration.is_empty() {
                                    continue;
                                }
                                self.agent_step(
                                    parent,
                                    AgentStep {
                                        id: format!("{message_id}:{index}"),
                                        kind: if thinking { "reasoning" } else { "message" }.into(),
                                        text: narration.into(),
                                        state: None,
                                        detail: String::new(),
                                        ..AgentStep::default()
                                    },
                                    model,
                                    events,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                Some("user") => {
                    for block in value["message"]["content"].as_array().into_iter().flatten() {
                        if block["type"] != "tool_result" {
                            continue;
                        }
                        let Some(id) = text(&block["tool_use_id"]) else {
                            continue;
                        };
                        let Some(tool) = self.tools.get_mut(parent) else {
                            continue;
                        };
                        let Some(run) = tool.agent_mut() else {
                            continue;
                        };
                        let Some(step) = run.steps.iter_mut().find(|step| step.id == id) else {
                            continue;
                        };
                        step.state = Some(if block["is_error"] == true {
                            ToolState::Failed
                        } else {
                            ToolState::Completed
                        });
                        step.detail = text_content(&block["content"]);
                        crate::tool::tasks::bound(&mut step.detail, 16 * 1024);
                        events.push(Event::Tool(tool.clone()));
                    }
                }
                Some("tool_progress") => self.claude_progress(value, parent, events),
                _ => {}
            }
            // Including nested stream deltas: the completed assistant snapshot
            // supplies stable message IDs and avoids duplicate streamed narration.
            return true;
        }
        if value["type"] == "system"
            && matches!(
                value["subtype"].as_str(),
                Some("task_started" | "task_progress" | "task_updated" | "task_notification")
            )
            && value["ambient"] != true
        {
            let subtype = value["subtype"].as_str().unwrap_or("");
            let patch = if subtype == "task_updated" {
                &value["patch"]
            } else {
                value
            };
            if let Some(id) = text(&value["task_id"]) {
                if patch["is_backgrounded"] == true {
                    self.activities.claude_background.insert(
                        id.into(),
                        text(&patch["description"])
                            .unwrap_or("Background task")
                            .into(),
                    );
                } else if patch["is_backgrounded"] == false {
                    self.activities.claude_background.remove(id);
                }
                if subtype == "task_started" {
                    self.activities.claude_pending.insert(id.into());
                }
                if subtype == "task_notification" {
                    let background = self.activities.claude_background.remove(id).is_some();
                    self.activities.claude_pending.remove(id);
                    if background && self.claude_result_seen {
                        // Claude may wake itself after a background result. The
                        // desktop adapter leaves a 15s window for that follow-up.
                        self.claude_resume_at = Some(Instant::now() + Duration::from_secs(15));
                    }
                }
            }
            let owner = text(&value["tool_use_id"]).map(str::to_owned).or_else(|| {
                text(&value["task_id"]).and_then(|id| self.activities.owners.get(id).cloned())
            });
            if let Some(tool) = owner.as_deref().and_then(|owner| self.tools.get_mut(owner)) {
                if tool.agent_mut().is_some() {
                    if let Some(id) = text(&value["task_id"]) {
                        if let Some(run) = tool.agent_mut() {
                            if !run.ids.iter().any(|old| old == id) {
                                run.ids.push(id.into());
                            }
                        }
                        self.activities.owners.insert(id.into(), tool.id.clone());
                    }
                    let subtype = value["subtype"].as_str().unwrap_or("");
                    let patch = if subtype == "task_updated" {
                        &value["patch"]
                    } else {
                        value
                    };
                    if patch["is_backgrounded"] == true {
                        self.activities.background_calls.insert(tool.id.clone());
                    } else if patch["is_backgrounded"] == false || subtype == "task_notification" {
                        self.activities.background_calls.remove(&tool.id);
                    }
                    if let Some(status) = patch["status"].as_str().and_then(state) {
                        tool.state = status;
                    } else if subtype == "task_started" {
                        tool.state = ToolState::Running;
                    }
                    if let Some(run) = tool.agent_mut() {
                        // Match React: keep the run's name once its trail starts.
                        if run.steps.is_empty() {
                            if let Some(description) = text(&patch["description"]) {
                                run.brief = description.into();
                            }
                        }
                        if let Some(summary) =
                            text(&value["summary"]).filter(|_| subtype == "task_progress")
                        {
                            if let Some(id) = text(&value["task_id"]) {
                                run.upsert(AgentStep {
                                    id: format!("task:{id}"),
                                    kind: "message".into(),
                                    text: summary.into(),
                                    state: None,
                                    detail: String::new(),
                                    ..AgentStep::default()
                                });
                            }
                        }
                    }
                    if subtype == "task_notification" {
                        if let Some(summary) = text(&value["summary"]) {
                            tool.output = summary.into();
                            tool.bound_output();
                        }
                    } else if let Some(error) = text(&patch["error"]) {
                        tool.output = error.into();
                        tool.bound_output();
                    }
                    events.push(Event::Tool(tool.clone()));
                }
            }
            return true;
        }
        false
    }
    fn claude_progress(&mut self, value: &Value, parent: &str, events: &mut Vec<Event>) {
        let (Some(id), Some(name)) = (text(&value["tool_use_id"]), text(&value["tool_name"]))
        else {
            return;
        };
        // Progress is a child's step, never the parent Agent's final report.
        self.agent_step(
            parent,
            AgentStep {
                id: id.into(),
                kind: "tool".into(),
                text: name.into(),
                state: Some(ToolState::Running),
                detail: String::new(),
                tool_kind: Some(name.into()),
                ..AgentStep::default()
            },
            None,
            events,
        );
    }
}
fn text_content(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

#[cfg(test)]
mod tests;
