//! Keep the history index independent of the much larger interactive chat state.
use super::*;

pub(super) struct Metadata<'a> {
    pub summary: &'a Session,
    pub busy: bool,
    live: Option<&'a Conversation>,
    draft: bool,
    remote_waiting: bool,
}
impl Metadata<'_> {
    pub(super) fn display_title(&self) -> String {
        vorkspace_core::session_title::display_title(&self.summary.title, &self.summary.provider)
            .replace('\n', " ")
    }
    pub(super) fn needs_approval(&self) -> bool {
        self.live.is_some_and(Conversation::needs_approval) || self.remote_waiting
    }
    pub(super) fn responding(&self) -> bool {
        self.live.is_some_and(Conversation::responding)
    }
    pub(super) fn has_saved_draft(&self) -> bool {
        self.live.map_or(self.draft, Conversation::has_saved_draft)
    }
}

impl Shell {
    pub(super) fn session_metadata<'a>(&'a self, id: i64, cx: &'a App) -> Metadata<'a> {
        let live = self.sessions.get(&id).map(|view| view.read(cx));
        Metadata {
            summary: live.map_or_else(|| &self.session_summaries[&id], |v| &v.summary),
            busy: live.is_some_and(|v| v.busy)
                || self.remote_marks.get(&id).is_some_and(|state| state.0),
            live,
            draft: self.draft_sessions.contains(&id),
            remote_waiting: self.remote_marks.get(&id).is_some_and(|state| state.1),
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
        let summary = self.session_summaries.get(&id)?.clone();
        self.add_loaded(summary, cx);
        self.sessions.get(&id).cloned()
    }
    pub(super) fn trim_session_histories(&mut self, shown: &HashSet<i64>, cx: &mut Context<Self>) {
        const INACTIVE_COUNT: usize = 8;
        const INACTIVE_BYTES: usize = 32 * 1024 * 1024;
        let preview = self.session_preview.as_ref().map(|p| p.id);
        let mut inactive: Vec<_> = self
            .sessions
            .values()
            .filter_map(|view| {
                let chat = view.read(cx);
                let id = chat.summary.id;
                (!shown.contains(&id) && preview != Some(id) && !chat.busy && chat.history_loaded())
                    .then(|| {
                        (
                            id,
                            self.history_recency.get(&id).copied().unwrap_or(0),
                            chat.history_weight(cx),
                        )
                    })
            })
            .collect();
        inactive.sort_unstable_by_key(|&(id, recency, _)| (recency, id));
        let mut count = inactive.len();
        let mut bytes = inactive.iter().map(|(_, _, weight)| *weight).sum::<usize>();
        for (id, _, weight) in inactive {
            if count <= INACTIVE_COUNT && bytes <= INACTIVE_BYTES {
                break;
            }
            let view = self.sessions.get(&id).cloned();
            if view.is_some_and(|view| view.update(cx, |chat, cx| chat.unload_history(cx))) {
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
