//! Composer policy and recovery for Host-owned conversations.
use super::*;
use serde_json::Value;
use vorkspace_core::agent::RemoteAction;
use vorkspace_core::remote::protocol::SessionObservation;
use vorkspace_ui::radius;

mod observation;

#[derive(Default)]
pub(super) struct State {
    pub(super) client: Option<Arc<vorkspace_core::remote::Connections>>,
    clock: Option<gpui::Task<()>>,
    pub(super) refreshing: bool,
    pub(super) awaiting_successor: bool,
    pub(super) refresh_generation: u64,
    pub(super) pending: bool,
    pub(super) online: bool,
    error: Option<String>,
    rejected: Option<Value>,
    observation: SessionObservation,
    pub(super) capabilities: std::collections::HashSet<String>,
}
impl State {
    fn legacy_recovery_error(&self) -> Option<&str> {
        self.rejected.as_ref().filter(|rejected| rejected["legacyRecovery"] == true)
            .map(|rejected| rejected["error"].as_str().unwrap_or("Older pending requests could not be linked to one conversation. Open the original desktop to recover them."))
    }
}

impl Conversation {
    #[cfg(test)]
    pub(crate) fn mark_remote_snapshot_loaded_for_test(&mut self) {
        debug_assert!(self.is_remote());
        self.loaded = true;
    }

    pub(super) fn is_remote(&self) -> bool {
        self.summary.cwd.starts_with("remote://")
    }
    pub(crate) fn remote_connection_error(&mut self, error: &str, cx: &mut Context<Self>) {
        if self.is_remote() && !self.responding() {
            self.remote_state.online = false;
            self.remote_state.error = Some(error.into());
            self.status = error.into();
            cx.notify();
        }
    }
    pub(super) fn load_remote(&mut self, cx: &mut Context<Self>) {
        if self.remote_state.clock.is_none() && !cfg!(test) {
            self.remote_state.clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(3))
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            // Live turns have their own 750 ms sync and control loop.
                            if !this.turn_in_flight() && !this.history_unloaded {
                                this.refresh_remote_snapshot(cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        if !self.loaded || self.history_unloaded {
            self.refresh_remote_snapshot(cx);
        }
    }
    fn install_remote_draft(&mut self, snapshot: &Value, cx: &mut Context<Self>) {
        let draft = snapshot["session"]["blocks"]
            .as_array()
            .and_then(|blocks| blocks.iter().find(|b| b["draft"] == true))
            .map(|block| vorkspace_core::follow_up::Message {
                id: block["id"].as_str().unwrap_or("").into(),
                text: block["text"].as_str().unwrap_or("").into(),
                at: self.transcript.read(cx).source_rows().len(),
                ..Default::default()
            });
        self.follow_ups.state.draft = draft.clone();
        self.transcript
            .update(cx, |view, cx| view.set_remote_saved_draft(draft, cx));
    }
    pub(super) fn remote_send_guard(&mut self, cx: &mut Context<Self>) -> bool {
        let error = if !self.loaded && self.remote_state.error.is_none() {
            Some("Wait for this remote conversation to load.")
        } else if !self.remote_state.online {
            Some("The machine is offline. Reconnect in Settings → Connections, then Retry.")
        } else if self.remote_state.pending {
            Some(self.remote_state.legacy_recovery_error().unwrap_or(
                "A remote command needs confirmation. Choose Retry before sending another message.",
            ))
        } else if self.turn_in_flight() {
            Some("Wait for this remote turn to finish before sending another message.")
        } else if self.remote_state.awaiting_successor {
            Some("Refreshing conversation…")
        } else {
            None
        };
        if let Some(error) = error {
            self.status = error.into();
            cx.notify();
            return true;
        }
        if self.settings.new_worktree && self.settings.worktree_cwd.is_none() {
            if !self.preparing_worktree {
                self.prepare_worktree(cx);
            }
            return true;
        }
        if !self.attachments.is_empty()
            && !self
                .remote_state
                .capabilities
                .contains("attachments.upload")
        {
            self.status =
                "Update vorkspace Host in Connections settings to send attachments.".into();
            cx.notify();
            return true;
        }
        if self.settings.plan && !self.remote_state.capabilities.contains("sessions.plan") {
            self.status = "Update vorkspace Host in Connections settings to use Plan mode.".into();
            cx.notify();
            return true;
        }
        if self.draft_active(cx) && !self.remote_state.capabilities.contains("sessions.draft") {
            self.status = "Update vorkspace Host in Connections settings to save drafts.".into();
            cx.notify();
            return true;
        }
        let text = self.input.read(cx).content.trim();
        if text == "/compact" {
            if self.launch_remote("/compact".into(), RemoteAction::Compact, cx) {
                self.input
                    .update(cx, |input, cx| input.set_text(String::new(), cx));
            }
            return true;
        }
        if text.starts_with('/')
            && !["/plan", "/draft"]
                .iter()
                .any(|command| text == *command || text.starts_with(&format!("{command} ")))
        {
            self.status =
                "Only /plan, /compact and saved drafts are supported by this remote host.".into();
            cx.notify();
            return true;
        }
        if self.settings.operator || self.settings.orchestrator {
            self.status = "Operator and orchestration are not available on remote projects.".into();
            cx.notify();
            return true;
        }
        false
    }
    pub(super) fn remote_draft_action(&mut self, send: bool, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) {
            return;
        }

        if self.turn_in_flight()
            || self.remote_state.pending
            || self.remote_state.awaiting_successor
            || !self.remote_state.online
        {
            return;
        }
        let Some(draft) = self.follow_ups.state.draft.as_ref() else {
            return;
        };
        let prompt = if send {
            draft.text.clone()
        } else {
            String::new()
        };
        let action = if send {
            RemoteAction::SendDraft(draft.id.clone())
        } else {
            RemoteAction::RemoveDraft(draft.id.clone())
        };
        self.launch_remote(prompt, action, cx);
    }
    pub(super) fn remote_save_draft(&mut self, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) {
            return;
        }

        if self.remote_send_guard(cx) {
            return;
        }
        let raw = self.input.read(cx).content.to_string();
        let prompt = vorkspace_core::composer::consume_command(&raw, "draft")
            .unwrap_or(&raw)
            .to_owned();
        if prompt.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        self.launch_composer(prompt, true, cx);
    }
    pub(super) fn remote_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.is_remote() || self.responding() {
            return None;
        }
        let legacy = self.remote_state.legacy_recovery_error();
        let restore = legacy.is_none() && self.remote_state.rejected.is_some();
        let retry = legacy.is_none()
            && (self.remote_state.pending || !self.remote_state.online || !restore);
        let message = if let Some(error) = legacy {
            error
        } else if self.remote_state.pending {
            "A remote command needs confirmation. Retry uses its original command ID."
        } else if let Some(error) = self.remote_state.error.as_deref() {
            error
        } else if restore {
            "Your previous message can be restored to the composer."
        } else {
            return None;
        };
        Some(
            div()
                .mx_2()
                .my_2()
                .p_3()
                .rounded(px(radius::CONTAINER))
                .bg(content(0.04))
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().text_size(px(12.)).child(message.to_owned()))
                .when(restore, |notice| {
                    notice.child(
                        crate::controls::secondary_button(
                            "remote-restore",
                            "Restore message",
                            true,
                            false,
                        )
                        .child("Restore message")
                        .on_click(cx.listener(|this, _, _, cx| this.restore_remote_message(cx))),
                    )
                })
                .when(retry, |notice| {
                    notice.child(
                        crate::controls::secondary_button("remote-retry", "Retry", true, false)
                            .debug_selector(|| "remote-retry".into())
                            .child("Retry")
                            .on_click(cx.listener(|this, _, _, cx| this.retry_remote(cx))),
                    )
                })
                .into_any_element(),
        )
    }
    fn retry_remote(&mut self, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) {
            return;
        }

        if let Some(error) = self.remote_state.legacy_recovery_error() {
            self.status = error.to_owned();
            cx.notify();
            return;
        }
        if self.turn_in_flight() {
            return;
        }
        if self.remote_state.pending {
            self.launch_remote(String::new(), RemoteAction::Retry, cx);
        } else {
            self.refresh_remote_snapshot(cx);
        }
    }
    fn restore_remote_message(&mut self, cx: &mut Context<Self>) {
        if self.reject_user_entry(cx) {
            return;
        }

        if let Some(error) = self.remote_state.legacy_recovery_error() {
            self.status = error.to_owned();
            cx.notify();
            return;
        }
        if !self.input.read(cx).content.trim().is_empty() || !self.attachments.is_empty() {
            self.status = "Clear the composer before restoring the rejected message.".into();
            cx.notify();
            return;
        }
        if let Some(rejected) = self.remote_state.rejected.as_ref() {
            let message = rejected["commands"]
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .find(|command| {
                    matches!(command["type"].as_str(), Some("send" | "draft" | "compact"))
                });
            if let Some(message) = message {
                let text = if message["type"] == "compact" {
                    "/compact"
                } else {
                    message["text"].as_str().unwrap_or("")
                };
                self.input
                    .update(cx, |input, cx| input.set_text(text.to_owned(), cx));
                self.attachments = rejected["uploads"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|upload| upload["path"].as_str().map(std::path::PathBuf::from))
                    .collect();
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
