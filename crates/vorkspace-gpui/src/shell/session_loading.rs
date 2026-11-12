//! Keep the history index independent of the much larger interactive chat state.
use super::*;

pub(super) struct Metadata<'a> {
    pub summary: &'a Session,
    pub busy: bool,
    live: Option<&'a Conversation>,
    draft: bool,
}
impl Metadata<'_> {
    pub(super) fn display_title(&self) -> String {
        vorkspace_core::session_title::display_title(&self.summary.title, &self.summary.provider)
            .replace('\n', " ")
    }
    pub(super) fn needs_approval(&self) -> bool {
        self.live.is_some_and(Conversation::needs_approval)
    }
    pub(super) fn responding(&self) -> bool {
        self.live
            .map_or(self.busy, |live| !live.is_stopping() && self.busy)
    }
    pub(super) fn has_saved_draft(&self) -> bool {
        self.live.map_or(self.draft, Conversation::has_saved_draft)
    }
}

impl Shell {
    pub(super) fn session_changed(
        &mut self,
        id: i64,
        change: SessionChanged,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.sessions.get(&id) else {
            return;
        };
        let view = view.read(cx);
        let record = self.session_record(id, cx);
        let previous = record.map(|record| &record.summary);
        let summary_changed = previous != Some(&view.summary);
        let provider_changed =
            previous.is_none_or(|summary| summary.provider != view.summary.provider);
        let project_changed = previous.is_none_or(|summary| summary.cwd != view.summary.cwd);
        let worktree = &view.settings.worktree_cwd;
        let checkout_changed = record.and_then(|row| row.checkout.as_ref()) != Some(worktree);
        let draft = view.has_saved_draft();
        let draft_changed = record.is_some_and(|row| row.draft) != draft;
        let needs_approval = view.needs_approval();
        if (summary_changed || checkout_changed || draft_changed)
            && self
                .app_core
                .as_ref()
                .is_none_or(|core| core.read(cx).session_installs.permits(id, None))
        {
            self.put_session_record(
                SessionRecord {
                    summary: view.summary.clone(),
                    checkout: Some(worktree.clone()),
                    draft,
                },
                cx,
            );
            self.publish_session_summary(id, cx);
        }

        use SessionChanged::*;
        let run_changed = matches!(change, Turn | Settled | Loaded);
        let attention_changed = change.affects_attention();
        let refresh_checkout = matches!(change, Checkout | Settled | Loaded)
            || matches!(change, Attention) && needs_approval;
        let active = self
            .workspace
            .active
            .and_then(|tab| self.workspace.session_for_pane(tab))
            == Some(id);
        if active && (checkout_changed || refresh_checkout || provider_changed || project_changed) {
            self.sync_git(cx);
            if matches!(change, Checkout) {
                self.git.update(cx, |git, cx| git.changed(cx));
            } else if refresh_checkout {
                self.git.update(cx, |git, cx| git.reload(cx));
            }
        }
        if summary_changed || checkout_changed || run_changed || matches!(change, History) {
            self.sync_search_session(id, cx);
        }
        if run_changed {
            self.orchestration_session_changed(id, cx);
            self.automation_session_changed(id, cx);
        }
        if run_changed || checkout_changed || matches!(change, Checkout) {
            self.sync_review_locks(cx);
        }
        if summary_changed || attention_changed {
            self.refresh_session_mark(id, cx);
        }
        if attention_changed {
            self.notification_session_changed(id, cx);
        }
        if active && (run_changed || summary_changed || matches!(change, Configuration)) {
            self.update_usage_provider(cx);
        }
        if run_changed || checkout_changed || project_changed || matches!(change, Checkout) {
            self.update_chat_visibility(cx);
        }
        if summary_changed
            || checkout_changed
            || draft_changed
            || attention_changed
            || matches!(change, Checkout)
        {
            self.invalidate_chrome(cx);
            cx.notify();
        }
    }

    pub(super) fn session_metadata<'a>(&'a self, id: i64, cx: &'a App) -> Metadata<'a> {
        let live = self.sessions.get(&id).map(|view| view.read(cx));
        Metadata {
            summary: live.map_or_else(
                || self.session_summary(id, cx).expect("available session"),
                |v| &v.summary,
            ),
            busy: live.is_some_and(Conversation::responding),
            live,
            draft: self.session_record(id, cx).is_some_and(|row| row.draft),
        }
    }
    /// Opening and explicit previews materialize chats; enumerating or scrolling
    /// history only reads summaries. Restored background tabs stay metadata-only.
    pub(super) fn ensure_session(
        &mut self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Conversation>> {
        if let Some(view) = self.sessions.get(&id) {
            return Some(view.clone());
        }
        let summary = self.session_summary(id, cx)?.clone();
        self.add_loaded(summary, cx)
    }
    pub(super) fn trim_session_histories(&self, cx: &mut Context<Self>) {
        const INACTIVE_COUNT: usize = 8;
        const INACTIVE_BYTES: usize = 32 * 1024 * 1024;
        let preview = self.preview.session_preview.as_ref().map(|p| p.id);
        let mut inactive: Vec<_> = self
            .sessions
            .iter()
            .filter(|&(&id, _)| {
                !self.visible_chats.contains(&id)
                    && preview != Some(id)
                    && !self.pages.automation_state.reserved.contains(&id)
            })
            .filter_map(|(&id, view)| {
                let chat = view.read(cx);
                chat.can_unload_history(cx).then(|| {
                    (
                        id,
                        self.history_recency.get(&id).copied().unwrap_or(0),
                        chat.history_weight(cx),
                    )
                })
            })
            .collect();
        // A single oversized conversation cannot displace the warm set.
        inactive.retain(|&(id, _, weight)| {
            if weight <= INACTIVE_BYTES {
                return true;
            }
            self.sessions[&id].update(cx, |chat, cx| chat.unload_history(cx));
            false
        });
        let mut count = inactive.len();
        let mut bytes = inactive
            .iter()
            .fold(0usize, |sum, &(_, _, weight)| sum.saturating_add(weight));
        if count <= INACTIVE_COUNT && bytes <= INACTIVE_BYTES {
            return;
        }
        inactive.sort_unstable_by_key(|&(id, recency, _)| (recency, id));
        for (id, _, weight) in inactive {
            if count <= INACTIVE_COUNT && bytes <= INACTIVE_BYTES {
                break;
            }
            if self.sessions[&id].update(cx, |chat, cx| chat.unload_history(cx)) {
                count -= 1;
                bytes = bytes.saturating_sub(weight);
            }
        }
    }
}

/// Preserve remembered source controls when they refer to the selected model.
/// A deliberate native model override starts with fresh model-specific controls.
pub(super) fn new_session_settings(
    source: Option<vorkspace_core::agent::Settings>,
    model: Option<String>,
) -> vorkspace_core::agent::Settings {
    let mut settings = source.unwrap_or_default();
    if let Some(model) = model {
        if settings
            .model
            .as_ref()
            .is_some_and(|source| source != &model)
        {
            settings.effort = None;
            settings.fast = false;
            settings.thinking = None;
            settings.service_tier = None;
            settings.model_settings.clear();
        }
        settings.model = Some(model);
    }
    settings
}

#[cfg(test)]
mod tests;
