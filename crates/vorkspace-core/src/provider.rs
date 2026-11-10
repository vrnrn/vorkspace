//! Native provider transports with bounded process and network output.
//! Blocking work belongs to a worker; never call `run` from a render/input handler.
pub(crate) mod copilot;
use crate::agent::{AccessMode, Approval, Control, Decision, Settings, TurnMetrics};
use crate::tool::{ToolCall, ToolState};
use crate::{process, transcript::snapshot_remainder};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

mod acp;
mod commands;
pub use commands::{
    compact, discover_commands, native_invocation, native_prompt, prepare_prompt, supports_compact,
    NativeCommand, NativeSubcommand,
};
mod attachments;
mod codex_text;
mod compaction;
mod follow_up;
mod frames;
mod lifecycle;
mod limits;
mod output;
mod parked;
mod pi;
pub fn configure_idle_agents(limit: usize) {
    parked::configure(limit);
}
pub fn keep_idle_agents(ids: Vec<i64>) {
    parked::keep(ids);
}
pub fn discard_worktree(path: &Path) {
    parked::discard_worktree(path);
}
pub fn forget_session(kind: Kind, provider_id: &str, account_id: &str, cwd: &Path) {
    parked::forget_session(kind, provider_id, account_id, cwd);
}
mod questions;
mod rewind;
mod runner;
mod tasks;
mod text_output;
pub use rewind::{rewind_last_turn, supports_rewind, RewindError};
pub(crate) use runner::catalog_for;
pub use runner::{
    run, run_restricted_text, run_text, run_text_with_timeout, run_with_controls,
    supports_text_only,
};
use runner::{run_operation, Operation};
#[cfg(test)]
mod text_tests;
pub use crate::cli::{resolve, resolve_auto};
pub use crate::kind::Kind;

const TICK: Duration = Duration::from_millis(16);

#[derive(Clone, Debug)]
pub struct Request {
    pub kind: Kind,
    pub binary: PathBuf,
    pub cwd: PathBuf,
    pub prompt: String,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub settings: Settings,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Ready,
    Commands(Vec<NativeCommand>),
    /// Synchronous admission before a follow-up can reach provider input.
    FollowUpDispatch(crate::follow_up::Message),
    FollowUp(crate::follow_up::Message),
    FollowUpFailed {
        id: String,
        error: String,
    },
    SessionId(String),
    TurnId(String),
    Model(String),
    Context {
        used: Option<u64>,
        window: Option<u64>,
    },
    Metrics(TurnMetrics),
    Approval(Approval),
    ApprovalResolved(String),
    Question(crate::agent::Question),
    QuestionResolved(String),
    BeginAssistant,
    Text(String),
    Reasoning(String),
    Tool(ToolCall),
    Status(String),
    UsageLimited(crate::limit::Limit),
    Interjection {
        text: String,
        custom_type: String,
        severity: Option<String>,
    },
    ConfigChanged {
        model: Option<String>,
        model_settings: HashMap<String, String>,
    },
    Done,
}

fn codex_error(error: &Value, fallback: &str) -> String {
    let message = error["message"].as_str().unwrap_or(fallback);
    if error["codexErrorInfo"] == "usageLimitExceeded" {
        format!("Usage limit reached: {message}")
    } else {
        message.into()
    }
}

fn write_json(out: &mut impl Write, value: Value) -> Result<(), String> {
    let mut frame = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    frame.push(b'\n');
    out.write_all(&frame)
        .and_then(|_| out.flush())
        .map_err(|e| e.to_string())
}

enum DecodedFrame {
    Events(Vec<Event>),
    ToolOutput { id: String, bytes: usize },
}

impl From<Vec<Event>> for DecodedFrame {
    fn from(events: Vec<Event>) -> Self {
        Self::Events(events)
    }
}

/// State machine is deliberately testable without spawning a provider.
struct Protocol {
    initialized: bool,
    compact: bool,
    command_probe: bool,
    compact_confirmed: bool,
    active_turn: Option<String>,
    previous_turn: Option<String>,
    interrupt_id: Option<String>,
    claude_prompt_sent: bool,
    claude_result_seen: bool,
    claude_usage_limit: Option<crate::limit::Limit>,
    claude_resume_at: Option<Instant>,
    follow_up_ready: bool,
    steering: HashMap<String, (crate::follow_up::Message, Instant)>,
    request: Request,
    emitted: HashMap<String, String>,
    claude_message: usize,
    begun: bool,
    done: bool,
    tools: HashMap<String, ToolCall>,
    tool_output: HashMap<String, output::OutputTail>,
    active_compaction: Option<String>,
    activities: tasks::Activities,
    /// Streaming tool id and its partial input JSON, by content block index.
    claude_tools: HashMap<u64, (String, String)>,
    text_key: Option<String>,
    approvals: HashMap<String, (Value, String, Value)>,
    metrics: TurnMetrics,
    usage_total: u64,
    acp: acp::State,
    pi: pi::State,
    probe: bool,
    catalog_result: Option<Value>,
    questions: HashMap<String, (Value, String, Value, crate::agent::Question)>,
    question_order: std::collections::VecDeque<String>,
    visible_question: Option<String>,
    async_question_items: std::collections::HashSet<String>,
    async_answers: HashMap<String, (Value, String, Value, crate::agent::Question)>,
    restricted_output: Option<text_output::Output>,
}
impl Protocol {
    fn new(request: Request) -> Self {
        Self {
            request,
            initialized: false,
            compact: false,
            command_probe: false,
            compact_confirmed: false,
            active_turn: None,
            previous_turn: None,
            interrupt_id: None,
            claude_prompt_sent: false,
            claude_result_seen: false,
            claude_usage_limit: None,
            claude_resume_at: None,
            follow_up_ready: false,
            steering: HashMap::new(),
            emitted: HashMap::new(),
            claude_message: 0,
            begun: false,
            done: false,
            tools: HashMap::new(),
            tool_output: HashMap::new(),
            active_compaction: None,
            activities: tasks::Activities::default(),
            claude_tools: HashMap::new(),
            text_key: None,
            approvals: HashMap::new(),
            metrics: TurnMetrics::default(),
            usage_total: 0,
            acp: Default::default(),
            pi: Default::default(),
            probe: false,
            catalog_result: None,
            questions: HashMap::new(),
            question_order: Default::default(),
            visible_question: None,
            async_question_items: Default::default(),
            async_answers: HashMap::new(),
            restricted_output: None,
        }
    }
    fn text(&mut self, key: &str, text: &str, snapshot: bool, events: &mut Vec<Event>) {
        if self.restricted_output.is_some() {
            return;
        }
        let already = self.emitted.entry(key.to_owned()).or_default();
        let text = if snapshot {
            snapshot_remainder(already, text)
        } else {
            text
        };
        if text.is_empty() {
            return;
        }
        already.push_str(text);
        let text = text.to_owned();
        if self.text_key.as_deref() != Some(key) {
            events.push(Event::BeginAssistant);
            self.begun = true;
            self.text_key = Some(key.to_owned());
        }
        events.push(Event::Text(text));
    }
    fn decode_frame(&mut self, v: Value, stdin: &mut impl Write) -> Result<DecodedFrame, String> {
        if self.request.kind == Kind::Codex {
            // Codex requests its clock before thread/start may have replied.
            if v["method"] == "currentTime/read" && v.get("id").is_some() {
                write_json(
                    stdin,
                    json!({"id":v["id"],"result":{"currentTimeAt":crate::time::now_ms()/1000}}),
                )?;
                return Ok(Vec::new().into());
            }
            let turn = v["params"]["turnId"]
                .as_str()
                .or_else(|| v["params"]["turn"]["id"].as_str());
            if turn.is_some() && turn == self.previous_turn.as_deref() {
                return Ok(Vec::new().into());
            }
            if let Some(id) = v["id"].as_str() {
                if let Some((wire, kind, request, question)) = self.async_answers.remove(id) {
                    if let Some(error) = v.get("error").filter(|e| !e.is_null()) {
                        let message = codex_error(error, "Could not send your answer to Codex");
                        self.questions
                            .insert(question.id.clone(), (wire, kind, request, question.clone()));
                        self.visible_question = Some(question.id.clone());
                        return Ok(vec![Event::Status(message), Event::Question(question)].into());
                    }
                    return Ok(vec![Event::QuestionResolved(question.id)].into());
                }
                if id.starts_with("native-answer-") || id.starts_with("native-interrupt-") {
                    return Ok(Vec::new().into());
                }
            }
            let method = v["method"].as_str().unwrap_or("");
            if self.previous_turn.is_some()
                && self.active_turn.is_none()
                && method != "turn/started"
                && (method.starts_with("item/") || method.starts_with("turn/") || method == "error")
            {
                return Ok(Vec::new().into());
            }
        }
        if self.request.kind == Kind::Claude {
            if v["type"] == "control_cancel_request" {
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
                return Ok(events.into());
            }
            if v["type"] == "control_response"
                || (v["type"] == "system"
                    && matches!(v["subtype"].as_str(), Some("init" | "initialized")))
            {
                self.claude_prompt(stdin)?;
            }
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
        let mut activity_events = Vec::new();
        if self.activity(&v, &mut activity_events) {
            return Ok(activity_events.into());
        }
        if let Some(output) = &mut self.restricted_output {
            output.receive(&v)?;
        }
        if let Some(events) = self.steer_response(&v) {
            return Ok(events.into());
        }
        let mut started_turn = None;
        if self.request.kind == Kind::Codex {
            if v["id"] == 2 {
                if let Some(id) = v["result"]["thread"]["id"].as_str() {
                    if self.compact && self.request.provider_id.as_deref() != Some(id) {
                        return Err("Codex resumed a different thread during compaction.".into());
                    }
                    self.request.provider_id = Some(id.into());
                }
            }
            if v["method"] == "turn/started" || v["id"] == 3 {
                let turn = if v["id"] == 3 {
                    &v["result"]["turn"]
                } else {
                    &v["params"]["turn"]
                };
                if let Some(id) = turn["id"].as_str() {
                    if self.active_turn.as_deref() != Some(id) {
                        started_turn = Some(id.to_owned());
                    }
                    self.active_turn = Some(id.into());
                }
            }
        }
        if (self.request.kind == Kind::Claude && v["type"] == "system" && v["subtype"] == "init")
            || (matches!(self.request.kind, Kind::Pi | Kind::Omp) && v["type"] == "agent_start")
        {
            self.follow_up_ready = true;
        }
        let mut events = activity_events;
        if let Some(id) = started_turn {
            events.push(Event::TurnId(id));
        }
        if self.request.kind == Kind::Claude && v["type"] == "system" {
            if v["status"] == "compacting" {
                self.compaction(None, ToolState::Running, &mut events);
            } else if v["subtype"]
                .as_str()
                .is_some_and(|subtype| subtype.starts_with("compact"))
            {
                self.compaction(None, ToolState::Completed, &mut events);
            }
        }
        if let Some(commands) = self.command_updates(&v) {
            events.push(Event::Commands(commands));
        }
        if self.request.kind == Kind::Codex {
            self.codex_async_questions(&v, &mut events, stdin)?;
        }
        if self.compact
            && self.request.kind == Kind::Claude
            && v["type"] == "system"
            && v["subtype"]
                .as_str()
                .is_some_and(|s| s.starts_with("compact"))
        {
            self.compact_confirmed = true;
        }
        match self.request.kind {
            Kind::Codex => {
                if let Some(error) = v.get("error").filter(|e| !e.is_null()) {
                    let message = codex_error(error, "RPC request failed");
                    if v["id"] == 2
                        && self.request.provider_id.is_some()
                        && lifecycle::recoverable_codex_resume(&message)
                    {
                        if self.compact {
                            return Err(
                                "The saved Codex context could not be resumed for compaction."
                                    .into(),
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
                            if self.request.settings.operator || self.request.settings.orchestrator
                            {
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
                    }
                    _ => {}
                }
                let p = &v["params"];
                match v["method"].as_str().unwrap_or("") {
                    "thread/tokenUsage/updated" => {
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
                            self.metrics.output_tokens +=
                                last["outputTokens"].as_u64().unwrap_or(0);
                            self.metrics.cache_read_tokens +=
                                last["cachedInputTokens"].as_u64().unwrap_or(0);
                            self.metrics.cache_write_tokens +=
                                last["cacheWriteInputTokens"].as_u64().unwrap_or(0);
                            if (last.get("cachedInputTokens").is_some()
                                || last.get("cacheWriteInputTokens").is_some())
                                && self.metrics.input_tokens > 0
                            {
                                self.metrics.cache_hit_percent = Some(
                                    self.metrics.cache_read_tokens as f64
                                        / (self.metrics.input_tokens
                                            + self.metrics.cache_write_tokens)
                                            as f64
                                        * 100.,
                                );
                            }
                            if self.metrics.reported() {
                                events.push(Event::Metrics(self.metrics));
                            }
                        }
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
                        let id = p["itemId"].as_str().unwrap_or("");
                        let tool = self
                            .tools
                            .entry(id.into())
                            .or_insert_with(|| ToolCall::new(id, "commandExecution"));
                        let delta = p["delta"].as_str().unwrap_or("");
                        self.tool_output
                            .entry(id.into())
                            .or_insert_with(|| {
                                output::OutputTail::from_output(std::mem::take(&mut tool.output))
                            })
                            .append(delta);
                        return Ok(DecodedFrame::ToolOutput {
                            id: id.into(),
                            bytes: delta.len(),
                        });
                    }
                    "turn/completed" | "turn/aborted" => {
                        let status = p["turn"]["status"].as_str().unwrap_or("completed");
                        if status == "failed" {
                            return Err(codex_error(&p["turn"]["error"], "Codex turn failed"));
                        }
                        let interrupted =
                            matches!(status, "interrupted" | "cancelled" | "canceled")
                                || v["method"] == "turn/aborted";
                        for item in p["turn"]["items"].as_array().into_iter().flatten() {
                            if interrupted && item["type"] == "contextCompaction" {
                                // An aborted turn can include its unfinished
                                // compaction item in the final snapshot.
                                continue;
                            }
                            self.codex_tool(item, true, &mut events);
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
                                || matches!(
                                    method,
                                    "execCommandApproval" | "applyPatchApproval"
                                )) =>
                    {
                        let detail = p["command"]
                            .as_str()
                            .or(p["reason"].as_str())
                            .map(str::to_owned)
                            .unwrap_or_default();
                        self.approval(
                            v["id"].clone(),
                            method,
                            p.clone(),
                            detail,
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
            }
            Kind::Claude => {
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
                        let e = &v["event"];
                        if e["type"] == "message_start" {
                            self.claude_message += 1;
                            self.claude_tools.clear();
                        }
                        if e["type"] == "content_block_start"
                            && e["content_block"]["type"] == "tool_use"
                        {
                            let block = &e["content_block"];
                            let id = block["id"].as_str().unwrap_or("");
                            let tool = ToolCall::new(id, block["name"].as_str().unwrap_or("Tool"));
                            self.claude_tools.insert(
                                e["index"].as_u64().unwrap_or(0),
                                (id.into(), String::new()),
                            );
                            self.tools.insert(id.into(), tool.clone());
                            events.push(Event::Tool(tool));
                        }
                        if e["type"] == "content_block_delta"
                            && e["delta"]["type"] == "input_json_delta"
                        {
                            // Like the desktop adapter, keep the last input that
                            // parses; a half-streamed object is not shown.
                            if let Some((id, partial)) =
                                self.claude_tools.get_mut(&e["index"].as_u64().unwrap_or(0))
                            {
                                partial.push_str(e["delta"]["partial_json"].as_str().unwrap_or(""));
                                let parsed = serde_json::from_str::<Value>(partial)
                                    .is_ok_and(|value| value.is_object());
                                if let Some(tool) =
                                    self.tools.get_mut(id.as_str()).filter(|_| parsed)
                                {
                                    tool.input = partial.clone();
                                    events.push(Event::Tool(tool.clone()));
                                }
                            }
                        }
                        if e["type"] == "content_block_delta"
                            && e["delta"]["type"] == "thinking_delta"
                        {
                            events.push(Event::Reasoning(
                                e["delta"]["thinking"].as_str().unwrap_or("").into(),
                            ));
                        }
                        if e["type"] == "content_block_delta" && e["delta"]["type"] == "text_delta"
                        {
                            self.text(
                                &self.claude_message.to_string(),
                                e["delta"]["text"].as_str().unwrap_or(""),
                                false,
                                &mut events,
                            );
                        }
                    }
                    "assistant" => {
                        if let Some(error) = v["error"].as_str() {
                            return Err(self.claude_error(error, &v));
                        }
                        if v["parent_tool_use_id"].is_null() {
                            if let Some(used) = crate::context::claude_used(&v["message"]["usage"])
                            {
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
                                tool.input = serde_json::to_string_pretty(&block["input"])
                                    .unwrap_or_default();
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
                        for block in v["message"]["content"].as_array().into_iter().flatten() {
                            if block["type"] == "tool_result" {
                                // A background Agent's launch receipt is not its final report.
                                if block["is_error"] != true
                                    && block["tool_use_id"].as_str().is_some_and(|id| {
                                        self.activities.background_calls.contains(id)
                                    })
                                {
                                    continue;
                                }
                                if let Some(tool) = block["tool_use_id"]
                                    .as_str()
                                    .and_then(|id| self.tools.get_mut(id))
                                {
                                    tool.output = block["content"]
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
                    "result" => {
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
                            return Err(self.claude_error(&error, &v));
                        }
                        // Some versions omit partial/assistant frames in simple turns.
                        if !self.begun {
                            self.text(
                                &self.claude_message.to_string(),
                                v["result"].as_str().unwrap_or(""),
                                true,
                                &mut events,
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
                                "claude",
                                request.clone(),
                                detail,
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
            }
            kind if kind.is_pi() => events.extend(self.pi_frame(v, stdin)?),
            _ => events.extend(self.acp_frame(v, stdin)?),
        }
        Ok(events.into())
    }
    #[cfg(test)]
    fn frame(&mut self, value: Value, stdin: &mut impl Write) -> Result<Vec<Event>, String> {
        match self.decode_frame(value, stdin)? {
            DecodedFrame::Events(events) => Ok(events),
            DecodedFrame::ToolOutput { id, .. } => Ok(self
                .tool_snapshot(&id)
                .map(Event::Tool)
                .into_iter()
                .collect()),
        }
    }
    fn tool_snapshot(&self, id: &str) -> Option<ToolCall> {
        let mut tool = self.tools.get(id)?.clone();
        if let Some(output) = self.tool_output.get(id) {
            tool.output = output.snapshot();
        }
        Some(tool)
    }

    fn approval(
        &mut self,
        wire: Value,
        kind: &str,
        request: Value,
        detail: String,
        stdin: &mut impl Write,
        events: &mut Vec<Event>,
    ) -> Result<(), String> {
        let id = wire.to_string();
        let title = if kind == "claude" {
            format!("Allow {}?", request["tool_name"].as_str().unwrap_or("tool"))
        } else if kind.contains("fileChange") || kind == "applyPatchApproval" {
            "Allow file changes?".into()
        } else if kind.contains("permissions") {
            "Allow additional permissions?".into()
        } else {
            "Allow this command?".into()
        };
        let mut detail =
            if let Some(tool) = request["itemId"].as_str().and_then(|id| self.tools.get(id)) {
                let detail = if detail.is_empty() {
                    tool.summary().1
                } else {
                    detail
                };
                let streamed = self
                    .tool_output
                    .get(&tool.id)
                    .map(output::OutputTail::snapshot);
                let output = streamed.as_deref().unwrap_or(&tool.output);
                if output.is_empty() {
                    detail
                } else {
                    format!("{detail}\n\n{output}")
                }
            } else {
                detail
            };
        if detail.is_empty() {
            detail = request
                .get("permissions")
                .and_then(|p| serde_json::to_string_pretty(p).ok())
                .unwrap_or_else(|| "The agent is requesting permission to continue.".into());
        }
        self.approvals
            .insert(id.clone(), (wire, kind.into(), request));
        if self.request.settings.plan || self.request.settings.access == AccessMode::ReadOnly {
            return self.respond(
                Control {
                    id,
                    decision: Decision::Deny,
                },
                stdin,
            );
        }
        let file_change = kind.contains("fileChange") || kind == "applyPatchApproval";
        let permissions = kind.contains("permissions");
        if self.request.settings.access == AccessMode::FullAccess
            || (self.request.kind == Kind::Codex
                && self.request.settings.access == AccessMode::AutoAcceptEdits
                && file_change)
            || (self.request.kind == Kind::Codex
                && permissions
                && matches!(
                    self.request.settings.access,
                    AccessMode::Auto | AccessMode::AutoAcceptEdits
                ))
        {
            return self.respond(
                Control {
                    id,
                    decision: Decision::Allow,
                },
                stdin,
            );
        }
        events.push(Event::Approval(Approval {
            id,
            title,
            detail,
            can_remember: self.request.kind != Kind::Codex && !file_change,
        }));
        Ok(())
    }
    fn respond(&mut self, control: Control, stdin: &mut impl Write) -> Result<(), String> {
        if matches!(control.decision, Decision::KeepOpen) {
            if let Some((_, _, _, question)) = self.questions.get_mut(&control.id) {
                question.auto_resolve_at = None;
            }
            return Ok(());
        }
        if let Some((wire, kind, request, mut question)) = self.questions.remove(&control.id) {
            if self.visible_question.as_deref() == Some(control.id.as_str()) {
                self.visible_question = None;
            }
            if kind == "codex-async" {
                if let Decision::Answer(answers) = &control.decision {
                    let text = question
                        .items
                        .iter()
                        .zip(answers)
                        .filter(|(_, a)| !a.is_empty())
                        .map(|(q, a)| format!("{}\n{}", q.prompt, a.join("\n")))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    if !text.is_empty() && request["turnId"].as_str() == self.active_turn.as_deref()
                    {
                        question.auto_resolve_at = None;
                        let id = format!("native-answer-{}-{}", question.id, crate::time::now_ms());
                        write_json(
                            stdin,
                            json!({"id":id,"method":"turn/steer","params":{"threadId":request["threadId"],"expectedTurnId":request["turnId"],"input":[{"type":"text","text":text}]}}),
                        )?;
                        self.async_answers
                            .insert(id, (wire, kind, request, question));
                    }
                }
                return Ok(());
            }
            return questions::respond(stdin, wire, &kind, &request, &question, control.decision);
        }
        let Some((wire, kind, request)) = self.approvals.remove(&control.id) else {
            return Ok(());
        };
        let allow = matches!(control.decision, Decision::Allow | Decision::Session);
        let session = matches!(control.decision, Decision::Session);
        if kind == "acp" {
            return acp::respond(stdin, wire, &request, control.decision);
        }
        if kind == "codex-mcp" {
            return write_json(
                stdin,
                json!({"id":wire,"result":{"action":if allow {"accept"} else {"decline"},"content":if allow {request["nativeContent"].clone()} else {Value::Null},"_meta":null}}),
            );
        }
        if kind == "pi" {
            return write_json(
                stdin,
                json!({"type":"extension_ui_response","id":wire,"confirmed":allow,"cancelled":!allow}),
            );
        }
        if kind == "claude" {
            let mut response = if allow {
                json!({"behavior":"allow","updatedInput":request["input"]})
            } else {
                json!({"behavior":"deny","message":"Denied by the user"})
            };
            if session && allow {
                if let Some(suggestions) = request.get("permission_suggestions") {
                    response["updatedPermissions"] = suggestions.clone();
                }
            }
            write_json(
                stdin,
                json!({"type":"control_response","response":{"subtype":"success","request_id":wire,"response":response}}),
            )
        } else {
            let result = if kind.contains("permissions") {
                json!({"permissions": if allow { request["permissions"].clone() } else { json!({}) }, "scope":if self.request.settings.access == AccessMode::FullAccess { "session" } else { "turn" }})
            } else if kind == "execCommandApproval" || kind == "applyPatchApproval" {
                json!({"decision":if !allow { "denied" } else if session { "approved_for_session" } else { "approved" }})
            } else {
                json!({"decision":if !allow { "decline" } else { "accept" }})
            };
            write_json(stdin, json!({"id":wire,"result":result}))
        }
    }
    fn codex_tool(&mut self, item: &Value, completed: bool, events: &mut Vec<Event>) {
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
            tool.input = serde_json::to_string(&json!({"path": changes.first().and_then(|c| c["path"].as_str()).unwrap_or("Files")})).unwrap_or_default();
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
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod process_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn fixture(script: &str) -> (tempfile::TempDir, Request) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("provider");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let request = Request {
            kind: Kind::Codex,
            binary: path,
            cwd: tmp.path().into(),
            prompt: "hello".into(),
            provider_id: None,
            model: None,
            settings: Default::default(),
        };
        (tmp, request)
    }
    #[test]
    fn cancellation_reaps_child_and_descendant_promptly() {
        for kind in Kind::ALL {
            let (_tmp, mut request) =
                fixture("#!/bin/sh\nsleep 30 &\nprintf ready > ready\nwait\n");
            request.kind = kind;
            request.settings.access = AccessMode::ReadOnly;
            let ready = request.cwd.join("ready");
            let cancel = Arc::new(AtomicBool::new(false));
            let stop = cancel.clone();
            let timer = thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !ready.exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                let started = ready.exists();
                let stopped_at = Instant::now();
                stop.store(true, Ordering::Relaxed);
                (started, stopped_at)
            });
            assert_eq!(run(request, cancel, |_| Ok(())).unwrap_err(), "Stopped");
            let (started, stopped_at) = timer.join().unwrap();
            assert!(started, "{} fixture did not start", kind.label());
            assert!(
                stopped_at.elapsed() < Duration::from_secs(2),
                "{} cancellation must not wait on provider output",
                kind.label()
            );
        }
    }
    #[test]
    fn crash_preserves_coalesced_partial_text() {
        let (_tmp, request) = fixture(
            "#!/bin/sh\nIFS= read -r init\nprintf '%s\\n' '{\"method\":\"item/agentMessage/delta\",\"params\":{\"itemId\":\"a\",\"delta\":\"partial\"}}'\n",
        );
        let mut text = String::new();
        let result = run(request, Arc::new(AtomicBool::new(false)), |events| {
            for event in events {
                if let Event::Text(t) = event {
                    text.push_str(&t);
                }
            }
            Ok(())
        });
        assert!(result.unwrap_err().contains("exited"));
        assert_eq!(text, "partial");
    }
    #[test]
    fn large_history_and_tool_frames_complete_the_turn() {
        let (_tmp, mut request) = fixture(
            r#"#!/usr/bin/env python3
import json, sys
payload = 'x' * (3 * 1024 * 1024) + 'end'
def send(value):
    print(json.dumps(value), flush=True)
for line in sys.stdin:
    value = json.loads(line)
    if value.get('id') == 1:
        send({'id': 1, 'result': {}})
    elif value.get('id') == 2:
        assert value['method'] == 'thread/resume'
        send({'id': 2, 'result': {'thread': {'id': 'large-thread', 'turns': [
            {'items': [{'id': 'history', 'type': 'agentMessage', 'text': payload}]}
        ]}}})
    elif value.get('id') == 3:
        send({'method': 'item/started', 'params': {'item': {
            'id': 'large-tool', 'type': 'commandExecution', 'command': 'large output', 'status': 'inProgress'
        }}})
        send({'method': 'item/completed', 'params': {'item': {
            'id': 'large-tool', 'type': 'commandExecution', 'status': 'completed', 'aggregatedOutput': payload
        }}})
        send({'method': 'item/agentMessage/delta', 'params': {'itemId': 'answer', 'delta': 'Complete'}})
        send({'method': 'turn/completed', 'params': {'turn': {'status': 'completed'}}})
"#,
        );
        request.provider_id = Some("large-thread".into());
        let mut text = String::new();
        let mut output = String::new();
        let mut done = false;
        run(request, Arc::new(AtomicBool::new(false)), |events| {
            for event in events {
                match event {
                    Event::Text(delta) => text.push_str(&delta),
                    Event::Tool(tool)
                        if tool.id == "large-tool" && tool.state == ToolState::Completed =>
                    {
                        output = tool.output;
                    }
                    Event::Done => done = true,
                    _ => {}
                }
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(text, "Complete");
        assert!(output.ends_with("end"));
        assert!(done);
    }

    #[test]
    fn malformed_provider_json_fails() {
        let (_tmp, request) = fixture("#!/bin/sh\nIFS= read -r init\nprintf '{invalid json\\n'\n");
        let error = run(request, Arc::new(AtomicBool::new(false)), |_| Ok(())).unwrap_err();
        assert!(error.starts_with("Invalid provider JSON:"), "{error}");
    }
}
