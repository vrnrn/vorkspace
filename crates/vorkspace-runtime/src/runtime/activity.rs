//! A small presentation receipt, produced where tools are already authoritative.
//! No output payload or transcript is retained by the sidebar projection.
use vorkspace_core::{tool::ToolCall, transcript::Row};

#[derive(Default)]
pub(super) struct ToolActivity {
    latest: Option<(usize, String)>,
    after_prompt: usize,
    source_end: usize,
    #[cfg(test)]
    inspected: usize,
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

    /// Remote history has already been projected on this worker. Inspect its
    /// changed suffix only; a rollback invalidating the receipt may also inspect
    /// the surviving prefix to recover its predecessor. Never called by a clock.
    pub(super) fn snapshot(&mut self, rows: &[Row], changed: usize, cwd: &str) {
        let changed = changed.min(rows.len());
        let invalidated = self.latest.as_ref().is_some_and(|(at, _)| {
            *at >= changed
                || changed < self.source_end
                || (changed == self.source_end && rows.get(changed).is_some_and(|row| !row.first))
        });
        let mut candidate = None;
        for at in (changed..rows.len()).rev() {
            #[cfg(test)]
            {
                self.inspected += 1;
            }
            if rows[at].first && (rows[at].is_prompt() || rows[at].role == "tool") {
                candidate = Some(at);
                break;
            }
        }
        if candidate.is_none() && invalidated {
            for at in (0..changed).rev() {
                #[cfg(test)]
                {
                    self.inspected += 1;
                }
                if rows[at].first && (rows[at].is_prompt() || rows[at].role == "tool") {
                    candidate = Some(at);
                    break;
                }
            }
        }
        if let Some(at) = candidate {
            self.latest = None;
            self.after_prompt = 0;
            if rows[at].is_prompt() {
                self.prompt(at);
            }
            if rows[at].role == "tool" {
                let end = (at + 1..rows.len())
                    .find(|&i| rows[i].first)
                    .unwrap_or(rows.len());
                self.source_end = end;
                let text: String = rows[at..end].iter().map(|row| row.text.as_str()).collect();
                if let Ok(tool) = serde_json::from_str::<ToolCall>(&text) {
                    self.tool(at, &tool, cwd);
                }
            }
        } else if invalidated {
            self.latest = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vorkspace_core::{tool::ToolState, transcript::Transcript};

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

    #[test]
    fn remote_suffix_chunks_replacements_and_rollbacks_match_the_authoritative_snapshot() {
        let mut transcript = Transcript::default();
        transcript.begin("user");
        transcript.append("Inspect");
        let first = transcript.begin("tool");
        transcript.append(&serde_json::to_string(&command("a", "cargo check")).unwrap());
        let second = transcript.begin("tool");
        transcript.append(&serde_json::to_string(&command("b", "cargo test")).unwrap());
        let mut activity = ToolActivity::default();
        activity.snapshot(&transcript.rows, 0, "/work");
        assert_eq!(activity.label().as_deref(), Some("Run cargo test"));
        // Replacement of the latest call, including a chunked JSON message.
        transcript.rows.truncate(second);
        let text = serde_json::to_string(&command("b", &"x".repeat(3000))).unwrap();
        transcript.begin("tool");
        transcript.append(&text);
        activity.snapshot(&transcript.rows, second, "/work");
        let before = activity.label();
        let mut changed = transcript.rows[second + 1].text.clone();
        changed = changed.replace('x', "y");
        transcript.rows[second + 1].text = changed;
        activity.snapshot(&transcript.rows, second + 1, "/work");
        // This replacement changes only a truncated-away command suffix.
        assert_eq!(activity.label(), before);
        transcript.rows.truncate(second);
        activity.snapshot(&transcript.rows, second, "/work");
        assert_eq!(activity.label().as_deref(), Some("Run cargo check"));
        transcript.rows.truncate(first);
        activity.snapshot(&transcript.rows, first, "/work");
        assert!(activity.label().is_none());
        activity.snapshot(&[], 0, "/work");
        assert!(activity.label().is_none());
    }

    #[test]
    fn incremental_text_never_revisits_old_history_or_retains_tool_output() {
        let mut rows = vec![
            Row {
                role: "assistant".into(),
                text: String::new(),
                first: true,
                sent_at: None
            };
            10_000
        ];
        let mut activity = ToolActivity::default();
        activity.snapshot(&rows, 0, "/work");
        let baseline = activity.inspected;
        for _ in 0..100 {
            let changed = rows.len();
            rows.push(Row {
                role: "assistant".into(),
                text: "token".into(),
                first: false,
                sent_at: None,
            });
            activity.snapshot(&rows, changed, "/work");
        }
        assert_eq!(activity.inspected - baseline, 100);
        assert!(activity.latest.is_none());
    }
}
