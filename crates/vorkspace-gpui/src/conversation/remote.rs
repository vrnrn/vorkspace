//! RemoteSession.tsx's host observation and composer restrictions.
use super::*;
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct State {
    pub(super) client: Option<Arc<vorkspace_core::remote::Connections>>,
    clock: Option<gpui::Task<()>>,
    pub(super) refreshing: bool,
    pub(super) refresh_generation: u64,
    pub(super) pending: bool,
    pub(super) online: bool,
    error: Option<String>,
    rejected: Option<Value>,
    snapshot: Option<Value>,
    pub(super) capabilities: std::collections::HashSet<String>,
}
impl State {
    fn legacy_recovery_error(&self) -> Option<&str> {
        self.rejected.as_ref().filter(|rejected| rejected["legacyRecovery"] == true)
            .map(|rejected| rejected["error"].as_str().unwrap_or("Older pending requests could not be linked to one conversation. Open the original desktop to recover them."))
    }
}
type Snapshot = (
    vorkspace_core::store::RemoteSession,
    vorkspace_core::agent::Settings,
    Option<Value>,
    Option<(
        crate::transcript::PreparedHistory,
        std::collections::HashMap<usize, vorkspace_core::agent::TurnInfo>,
    )>,
    Option<Value>,
);

impl Conversation {
    pub(super) fn is_remote(&self) -> bool {
        self.summary.cwd.starts_with("remote://")
    }
    pub(crate) fn remote_connection_error(&mut self, error: &str, cx: &mut Context<Self>) {
        if self.is_remote() && !self.busy {
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
                            if !this.busy && !this.stopping && !this.history_unloaded {
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
    pub(super) fn refresh_remote_snapshot(&mut self, cx: &mut Context<Self>) {
        if self.remote_state.refreshing || self.busy || self.stopping {
            return;
        }
        self.remote_state.refreshing = true;
        self.remote_state.refresh_generation = self.remote_state.refresh_generation.wrapping_add(1);
        let refresh = self.remote_state.refresh_generation;
        let submission = self.submission_generation;
        let initial = !self.loaded || self.history_unloaded;
        self.loading = initial;
        let store = self.store.clone();
        let id = self.summary.id;
        let known = self.remote_state.snapshot.clone();
        if self.remote_state.client.is_none() {
            self.remote_state.client = vorkspace_core::remote::default_connections().ok();
        }
        let client = self.remote_state.client.clone();
        let work = cx.background_executor().spawn(async move {
            let (mut remote, mut settings) = {
                let store = locked(&store)?;
                (
                    store
                        .remote_session(id)?
                        .ok_or("Remote session no longer exists")?,
                    store.agent_settings(id)?.unwrap_or_default(),
                )
            };
            let client = client.ok_or("Remote connections are unavailable")?;
            let machine = remote.machine(&client)?;
            let descriptor = if initial {
                Some(client.request(&machine.id, "environment.describe", json!({}))?)
            } else {
                None
            };
            let snapshot = remote
                .session_id
                .as_deref()
                .map(|session| client.load_session(&machine.id, session, known.as_ref()))
                .transpose()?;
            if let Some(snapshot) = snapshot.as_ref() {
                let store = locked(&store)?;
                let summary = store.register_remote_summary(&remote.project, snapshot)?;
                remote = store
                    .remote_session(summary.id)?
                    .ok_or("Remote session identity is missing")?;
                settings = store.agent_settings(summary.id)?.unwrap_or_default();
            }
            let changed = initial
                || match snapshot.as_ref() {
                    Some(snapshot) => vorkspace_runtime::runtime::remote::snapshot_changed(
                        known.as_ref(),
                        snapshot,
                    ),
                    None => known.is_some(),
                };
            let history = if changed {
                let qualified = snapshot.as_ref().map(|snapshot| {
                    let mut snapshot = snapshot.clone();
                    if let Some(id) = remote.session_id.as_deref() {
                        vorkspace_core::remote::attachments::qualify_snapshot(
                            &mut snapshot,
                            &remote.project.environment_id,
                            id,
                        );
                    }
                    snapshot
                });
                let (history, turns) = qualified
                    .as_ref()
                    .map(vorkspace_core::store::Store::project_remote_history)
                    .transpose()?
                    .unwrap_or_default();
                Some((
                    crate::transcript::PreparedHistory::new(history.rows, false),
                    turns,
                ))
            } else {
                None
            };
            Ok::<_, String>((remote, settings, snapshot, history, descriptor))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_remote_snapshot(refresh, submission, initial, result, cx);
            });
        })
        .detach();
    }
    fn finish_remote_snapshot(
        &mut self,
        refresh: u64,
        submission: u64,
        initial: bool,
        result: Result<Snapshot, String>,
        cx: &mut Context<Self>,
    ) {
        // A completed turn can already own a newer refresh. An older read
        // must not clear its admission flag or replace its visible state.
        if self.remote_state.refresh_generation != refresh {
            return;
        }
        self.remote_state.refreshing = false;
        if self.submission_generation != submission || self.busy || self.stopping {
            return;
        }
        if result.as_ref().is_ok_and(|(remote, _, snapshot, _, _)| {
            remote.host_revision.is_some_and(|latest| {
                snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot["revision"].as_u64())
                    .is_none_or(|received| received < latest)
            })
        }) {
            // Another observation already committed a newer host revision.
            // Its metadata cannot be paired with this older transcript.
            if initial {
                self.loading = false;
                let error = "The host returned an older conversation snapshot. Retry to load the latest version.".to_owned();
                self.status = error.clone();
                self.remote_state.error = Some(error);
                cx.notify();
            }
            return;
        }
        self.loading = false;
        match result {
            Ok((remote, settings, snapshot, history, descriptor)) => {
                self.remote_state.online = true;
                self.remote_state.error = remote
                    .rejected
                    .as_ref()
                    .and_then(|rejected| rejected["error"].as_str().map(str::to_owned));
                self.remote_state.rejected = remote.rejected;
                self.remote_state.pending = !remote.outbox.is_empty()
                    || self.remote_state.legacy_recovery_error().is_some();
                self.summary = remote.summary;
                if let Some(descriptor) = descriptor {
                    self.remote_state.capabilities = descriptor["capabilities"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect();
                }
                if settings.revision > self.settings.revision
                    || (initial && settings.revision >= self.settings.revision)
                {
                    self.settings = settings;
                    self.model = self.settings.model.clone();
                }
                if let Some((history, turns)) = history {
                    self.settings.turns = turns.clone();
                    self.transcript.update(cx, |view, cx| {
                        view.turns = turns;
                        view.install_prepared_history(history, cx);
                    });
                }
                self.loaded = true;
                self.history_unloaded = false;
                self.status = "Ready".into();
                let running = snapshot.as_ref().is_some_and(|s| s["status"] == "running");
                if let Some(snapshot) = &snapshot {
                    self.context = snapshot["session"]
                        .get("context")
                        .cloned()
                        .and_then(|context| serde_json::from_value(context).ok());
                    if let Some(title) = snapshot["session"]["title"].as_str() {
                        self.summary.title = title.into();
                    }
                    self.summary.provider_id = snapshot["session"]["providerSessionId"]
                        .as_str()
                        .map(str::to_owned);
                    self.summary.has_messages = snapshot["session"]["blocks"]
                        .as_array()
                        .is_some_and(|b| !b.is_empty());
                    self.install_remote_draft(snapshot, cx);
                    self.install_remote_plans(snapshot, cx);
                }
                self.remote_state.snapshot = snapshot;
                if initial {
                    self.refresh_catalog(cx);
                    self.refresh_git(cx);
                }
                if let Some((row, query)) = self.pending_search.take() {
                    self.reveal_search(row, query, cx);
                }
                if running && !self.busy && !self.stopping {
                    self.settings.remote_action = Some(json!({"type":"attach"}));
                    self.launch_prompt(String::new(), cx);
                } else {
                    self.run_history_actions(cx);
                }
            }
            Err(error) => self.remote_connection_error(&error, cx),
        }
        cx.emit(SessionChanged);
        cx.notify();
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
        let error = if !self.loaded {
            Some("Wait for this remote conversation to load.")
        } else if !self.remote_state.online {
            Some("The machine is offline. Reconnect in Settings → Connections, then Retry.")
        } else if self.remote_state.pending {
            Some(self.remote_state.legacy_recovery_error().unwrap_or(
                "A remote command needs confirmation. Choose Retry before sending another message.",
            ))
        } else if self.busy || self.stopping {
            Some("Wait for this remote turn to finish before sending another message.")
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
            self.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            self.launch_prompt("/compact".into(), cx);
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

        if self.busy || self.stopping || self.remote_state.pending || !self.remote_state.online {
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
        self.settings.remote_action = Some(if send {
            json!({"type":"send","draftBlockId":draft.id,"intent":"default"})
        } else {
            json!({"type":"removeDraft","draftBlockId":draft.id})
        });
        self.launch_prompt(prompt, cx);
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
        self.input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.follow_ups.draft_selected = false;
        self.settings.remote_action = Some(json!({"type":"draft"}));
        self.launch_prompt(prompt, cx);
    }
    pub(super) fn remote_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.is_remote()
            || self.busy
            || (self.remote_state.error.is_none() && !self.remote_state.pending)
        {
            return None;
        }
        let legacy = self.remote_state.legacy_recovery_error();
        Some(div().mx_2().my_2().p_3().rounded_lg().bg(content(0.04)).flex().items_center().gap_2()
            .child(div().flex_1().text_size(px(12.)).child(if let Some(error) = legacy {error.to_owned()} else if self.remote_state.pending {"A remote command needs confirmation. Retry uses its original command ID.".to_owned()} else {self.remote_state.error.clone().unwrap_or_default()}))
            .when(legacy.is_none(), |d| d
            .child(crate::controls::secondary_button("remote-retry",if self.remote_state.rejected.is_some() {"Restore message"} else {"Retry"},true,false).debug_selector(|| "remote-retry".into()).child(if self.remote_state.rejected.is_some() {"Restore message"} else {"Retry"}).on_click(cx.listener(|this,_,_,cx| {
                if this.remote_state.rejected.is_some() { this.restore_remote_message(cx); } else {this.retry_remote(cx);}
            })))).into_any_element())
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
        if self.busy || self.stopping {
            return;
        }
        if self.remote_state.pending {
            self.settings.remote_action = Some(json!({"type": "retry"}));
            self.launch_prompt(String::new(), cx);
        } else {
            self.refresh_remote_snapshot(cx);
        }
    }
    pub(super) fn release_remote_snapshot(&mut self) {
        self.remote_state.snapshot = None;
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
