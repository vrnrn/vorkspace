//! A small presentation receipt, produced where tools are already authoritative.
//! No output payload or transcript is retained by the sidebar projection.
use vorkspace_core::tool::ToolCall;

#[derive(Default)]
pub(super) struct ToolActivity {
    latest: Option<(usize, String)>,
    after_prompt: usize,
}
impl ToolActivity {
    pub(super) fn label(&self) -> Option<String> {
        self.latest.as_ref().map(|(_, label)| label.clone())
    }

    pub(super) fn prompt(&mut self, at: usize) {
        self.latest = None;
        self.after_prompt = at.saturating_add(1);
    }

    pub(super) fn tool(&mut self, at: usize, tool: &ToolCall, cwd: &str) {
        if at < self.after_prompt {
            return;
        }
        // A late completion/output for an older parallel call cannot replace
        // the most recently admitted tool, matching transcript message order.
        if self.latest.as_ref().is_some_and(|(latest, _)| *latest > at) {
            return;
        }
        let shown = tool.presentation_in(Some(cwd));
        let label = [shown.action, shown.target]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        // Commands and provider display metadata can be arbitrarily long.
        let mut chars = label
            .chars()
            .map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c });
        let mut label: String = chars.by_ref().take(239).collect();
        if chars.next().is_some() {
            label.push('…');
        }
        self.latest = (!label.is_empty()).then_some((at, label));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vorkspace_core::tool::ToolState;

    fn command(id: &str, command: &str) -> ToolCall {
        let mut tool = ToolCall::new(id, "exec_command");
        tool.input = serde_json::json!({"cmd":command}).to_string();
        tool
    }

    #[test]
    fn actual_tool_presentation_is_bounded_and_parallel_completions_keep_message_order() {
        let mut activity = ToolActivity::default();
        let read = command("a", "cargo check");
        let edit = command("b", "cargo test");
        activity.tool(1, &read, "/work");
        assert_eq!(activity.label().as_deref(), Some("Run cargo check"));
        activity.tool(2, &edit, "/work");
        let mut done = read;
        done.state = ToolState::Completed;
        done.output = "x".repeat(64 * 1024);
        activity.tool(1, &done, "/work");
        assert_eq!(activity.label().as_deref(), Some("Run cargo test"));
        activity.tool(2, &command("b", &"é\n".repeat(500)), "/work");
        let label = activity.label().unwrap();
        assert!(label.chars().count() <= 240 && !label.contains('\n'));
        activity.prompt(3);
        activity.tool(2, &done, "/work");
        assert!(
            activity.label().is_none(),
            "a steer must not inherit the previous prompt's tool"
        );
    }
}
