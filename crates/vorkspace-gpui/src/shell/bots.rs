use super::*;
use vorkspace_core::agent::Settings;
pub(crate) struct BotSession {
    pub id: i64,
    pub title: String,
    pub cwd: String,
    pub busy: bool,
    pub has_messages: bool,
    pub archived: bool,
    pub updated_at: u64,
    pub needs_input: bool,
    pub branch: Option<String>,
    pub model: SharedString,
}
impl Shell {
    #[cfg(test)]
    pub(crate) fn bot_manager(&self) -> Entity<crate::bots::Bots> {
        self.bots.as_ref().unwrap().clone()
    }
    #[cfg(all(target_os = "macos", not(test)))]
    pub(crate) fn bot_quick_compose(&mut self, cx: &mut Context<Self>) {
        self.create(Kind::Codex, cx);
    }
    pub(super) fn bot_active(&self, cx: &App) -> bool {
        self.bots
            .as_ref()
            .is_some_and(|m| m.read(cx).active.is_some())
    }
    pub(crate) fn bot_session_views(&self, cx: &App) -> Vec<BotSession> {
        self.order
            .iter()
            .map(|&id| {
                let metadata = self.session_metadata(id, cx);
                let live = self.sessions.get(&id).map(|v| v.read(cx));
                BotSession {
                    id,
                    title: metadata.summary.title.clone(),
                    cwd: metadata.summary.cwd.clone(),
                    busy: metadata.busy,
                    has_messages: metadata.summary.has_messages,
                    archived: metadata.summary.archived,
                    updated_at: metadata.summary.updated_at,
                    needs_input: metadata.needs_approval(),
                    branch: live.and_then(|v| v.branch_name()).map(str::to_owned),
                    model: live.map(|v| v.bot_model_label()).unwrap_or_default(),
                }
            })
            .collect()
    }
    pub(crate) fn bot_session_entity(&self, id: i64) -> Option<Entity<Conversation>> {
        self.sessions.get(&id).cloned()
    }
    pub(crate) fn bot_ensure_session(
        &mut self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Conversation>> {
        self.ensure_session(id, cx)
    }
    pub(crate) fn bot_install(
        &mut self,
        summary: Session,
        settings: Settings,
        prompt: String,
        draft: bool,
        cx: &mut Context<Self>,
    ) {
        let id = summary.id;
        self.add_loaded(summary, cx);
        let view = self.sessions[&id].clone();
        view.update(cx, |v, cx| {
            v.install_transfer(settings, (!draft).then(|| prompt.clone()), cx);
            if draft {
                v.input.update(cx, |i, cx| i.set_text(prompt, cx));
            }
        });
        self.invalidate_chrome(cx);
    }
    pub(crate) fn bot_groups(&self, project: &str) -> vorkspace_core::session_groups::Groups {
        self.groups.get(project).cloned().unwrap_or_default()
    }
    pub(crate) fn bot_apply_groups(
        &mut self,
        project: String,
        groups: vorkspace_core::session_groups::Groups,
        cx: &mut Context<Self>,
    ) {
        self.groups.insert(project, groups);
        self.invalidate_chrome(cx);
    }
}
