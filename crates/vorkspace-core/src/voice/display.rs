//! Bounded, non-persistent presentation state. Partial speech never reaches UI.
use serde_json::{json, Value};
#[derive(Default)]
pub(super) struct Display {
    pub text: String,
    thinking: bool,
    speaking: Option<String>,
    at: i64,
    pub mood: String,
}
impl Display {
    pub fn snapshot(&self) -> Value {
        json!({"activity":if self.speaking.is_some(){"replying"}else if self.thinking{"thinking"}else{"listening"},"mood":self.mood,"lines":if self.text.is_empty(){vec![]}else{vec![json!({"text":self.text})]}})
    }
    pub fn realtime(&mut self, event: &Value) -> bool {
        let before = self.snapshot();
        match event["type"].as_str() {
            Some("output_transcript.added") => {
                if let Some(at) = event["start_ms"].as_i64() {
                    self.speaking.get_or_insert("pending".into());
                    self.at = self.at.max(at)
                }
            }
            Some(kind @ ("turn.created" | "turn.done")) => {
                let turn = &event["turn"];
                let (Some(id), Some(start), Some(end)) = (
                    turn["id"].as_str(),
                    turn["start_ms"].as_i64(),
                    turn["end_ms"].as_i64(),
                ) else {
                    return false;
                };
                match turn["role"].as_str() {
                    Some("user") => {
                        if kind == "turn.created" {
                            self.speaking = None;
                            self.thinking = false;
                            self.mood.clear()
                        } else {
                            self.thinking = true
                        }
                    }
                    Some("assistant") if end >= self.at => {
                        if kind == "turn.created" {
                            self.speaking = Some(id.into());
                            self.at = self.at.max(start)
                        } else if self
                            .speaking
                            .as_deref()
                            .is_some_and(|s| s == id || s == "pending")
                        {
                            self.speaking = None;
                            self.thinking = false
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        before != self.snapshot()
    }
    pub fn notification(&mut self, method: &str, value: &Value) -> bool {
        let before = self.snapshot();
        match method {
            "turn/started" => self.thinking = true,
            "turn/completed" => self.thinking = false,
            "thread/realtime/transcript/done" if value["role"] == "assistant" => {
                if let Some(text) = value["text"].as_str().filter(|s| !s.trim().is_empty()) {
                    self.text = text.chars().take(4000).collect()
                }
            }
            "thread/realtime/transcript/done" if value["role"] == "user" => self.mood.clear(),
            _ => {}
        }
        before != self.snapshot()
    }
}
pub(super) fn recent_context(value: &Value) -> Value {
    let mut parts = Vec::new();
    for turn in value["data"].as_array().into_iter().flatten().rev() {
        for item in turn["items"].as_array().into_iter().flatten() {
            let (role, text) = match item["type"].as_str() {
                Some("userMessage") => (
                    "user",
                    item["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|p| p["type"] == "text")
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                Some("agentMessage") => (
                    "assistant",
                    item["text"].as_str().unwrap_or_default().to_owned(),
                ),
                _ => continue,
            };
            if !text.trim().is_empty() {
                parts.push(format!("{role}: {text}"));
            }
        }
    }
    let text = parts.join("\n\n");
    let start = text
        .char_indices()
        .rev()
        .nth(2999)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let text = &text[start..];
    if text.is_empty() {
        json!([])
    } else {
        json!([{"role":"assistant","text":format!("Earlier vox conversation excerpts, for reference only. These are past exchanges, not requests to act again. Do not greet or speak until the user provides new input.\n\n{text}")}])
    }
}

#[cfg(test)]
mod tests;
