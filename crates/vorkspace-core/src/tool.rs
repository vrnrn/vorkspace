//! Provider-neutral tool presentation, persisted as a native `tool` row.
use serde::{Deserialize, Serialize};
mod presentation;
pub mod tasks;
pub use presentation::{CommandStep, ToolPresentation};

pub(crate) const OUTPUT_LIMIT: usize = 64 * 1024;
pub(crate) const OUTPUT_OMISSION: &str = "… earlier output omitted …\n";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: String,
    pub output: String,
    pub state: ToolState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<tasks::Panel>,
    /// Descriptive provider/desktop metadata, separate from actual arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<serde_json::Value>,
}
impl ToolCall {
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            input: String::new(),
            output: String::new(),
            state: ToolState::Running,
            panel: None,
            display: None,
        }
    }
    pub fn is_compaction(&self) -> bool {
        self.name == "contextCompaction"
    }
    pub(crate) fn append_output(&mut self, delta: &str) {
        self.output.push_str(delta);
        self.bound_output();
    }
    pub(crate) fn bound_output(&mut self) {
        if let Some(tail) = self.output_tail() {
            self.output = format!("{OUTPUT_OMISSION}{tail}");
        }
    }

    /// Compare against the projected output without copying the full source.
    pub(crate) fn bounded_output_matches(&self, other: &str) -> bool {
        match self.output_tail() {
            Some(tail) => other.strip_prefix(OUTPUT_OMISSION) == Some(tail),
            None => self.output == other,
        }
    }

    fn output_tail(&self) -> Option<&str> {
        if self.output.len() <= OUTPUT_LIMIT {
            return None;
        }
        let at = self
            .output
            .ceil_char_boundary(self.output.len() - OUTPUT_LIMIT);
        Some(&self.output[at..])
    }
    pub fn summary(&self) -> (String, String) {
        match self.panel().as_deref() {
            Some(tasks::Panel::Tasks(list)) => return ("Tasks".into(), list.progress()),
            Some(tasks::Panel::Agent(run)) => return ("Agent".into(), run.title().into()),
            None => {}
        }
        let shown = self.presentation();
        (shown.action, shown.target)
    }
}

impl ToolCall {
    /// Provider metadata affects display only. The persisted arguments stay intact.
    pub fn presentation(&self) -> ToolPresentation {
        presentation::present(self, None)
    }
    pub fn presentation_in(&self, cwd: Option<&str>) -> ToolPresentation {
        presentation::present(self, cwd)
    }
    /// Show the provider's actual input rather than the desktop adapter envelope.
    pub fn input_detail(&self) -> String {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.input) else {
            return self.input.clone();
        };
        let raw = if value["__desktop"].is_object() {
            value
                .get("rawInput")
                .filter(|raw| !raw.is_null())
                .unwrap_or(&value["__desktop"])
        } else {
            &value
        };
        raw.as_str().map(str::to_owned).unwrap_or_else(|| {
            serde_json::to_string_pretty(raw).unwrap_or_else(|_| self.input.clone())
        })
    }
}
