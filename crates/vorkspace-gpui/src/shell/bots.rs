use super::*;
pub(crate) use crate::bots::host_api::BotSession;
use crate::bots::host_api::BotsHost;
use serde_json::Value;
use vorkspace_core::agent::Settings;
use vorkspace_core::{project_appearance::ProjectAppearance, session_groups::Groups};
impl Shell {
    pub(super) fn close_bot_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Park the page, retaining Bot/channel drafts and in-flight work.
        if let Some(bots) = self.bots.clone() {
            bots.update(cx, |bots, cx| bots.close(cx));
        }
        self.close_app_view(window, cx);
    }
    #[cfg(test)]
    pub(crate) fn bot_manager(&self) -> Entity<crate::bots::Bots> {
        self.bots.as_ref().unwrap().clone()
    }
    pub(super) fn bot_active(&self, cx: &App) -> bool {
        self.bots
            .as_ref()
            .is_some_and(|m| m.read(cx).active.is_some() || m.read(cx).classic_channels_open())
    }
    /// Channels cover the workspace too, but never own a Bot's details pane.
    pub(super) fn bot_details_active(&self, cx: &App) -> bool {
        self.bots.as_ref().is_some_and(|bots| {
            let bots = bots.read(cx);
            bots.active
                .as_deref()
                .is_some_and(|id| !bots.minion_read_only(id))
                && !bots.classic_channels_open()
        })
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
        self.sessions.get(&id).cloned().or_else(|| {
            self.bot_conversations
                .borrow()
                .get(&id)
                .and_then(|v| v.upgrade())
        })
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
        // Bot callers can still hold their entity while applying these groups.
        // Keep state current, but refresh only after that update has returned.
        let owner = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |owner, cx| owner.invalidate_chrome(cx));
        });
    }
}

/// The shell's half of the contract `Bots` depends on.
impl BotsHost for gpui::WeakEntity<Shell> {
    fn terminals_using(&self, path: &str, cx: &App) -> bool {
        self.upgrade()
            .is_some_and(|shell| shell.read(cx).terminals_using(path, cx))
    }
    fn session_views(&self, cx: &App) -> Vec<BotSession> {
        self.upgrade()
            .map(|shell| shell.read(cx).bot_session_views(cx))
            .unwrap_or_default()
    }
    fn session_entity(&self, id: i64, cx: &App) -> Option<Entity<Conversation>> {
        self.upgrade()
            .and_then(|shell| shell.read(cx).bot_session_entity(id))
    }
    fn ensure_session(&self, id: i64, cx: &mut App) -> Option<Entity<Conversation>> {
        self.upgrade()
            .and_then(|shell| shell.update(cx, |shell, cx| shell.bot_ensure_session(id, cx)))
    }
    fn groups(&self, project: &str, cx: &App) -> Groups {
        self.upgrade()
            .map(|shell| shell.read(cx).bot_groups(project))
            .unwrap_or_default()
    }
    fn apply_groups(&self, project: String, groups: Groups, cx: &mut App) {
        let _ = self.update(cx, |shell, cx| shell.bot_apply_groups(project, groups, cx));
    }
    fn placement(
        &self,
        caller: i64,
        project: &str,
        input: &Value,
        cx: &App,
    ) -> Result<Option<(i64, Edge)>, String> {
        self.upgrade()
            .ok_or_else(|| "Workspace is unavailable".to_string())
            .and_then(|shell| shell.read(cx).app_placement(caller, project, input, cx))
    }
    fn install_start(
        &self,
        caller: i64,
        start: AppStart,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Result<Result<i64, String>> {
        self.update(cx, |shell, cx| {
            shell.app_install_start(caller, start, window, cx)
        })
    }
    fn session_action(
        &self,
        caller: i64,
        id: i64,
        project: &str,
        action: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Result<Result<Value, String>> {
        self.update(cx, |shell, cx| {
            shell.app_session_action(caller, id, project, action, window, cx)
        })
    }
    fn open_session(&self, id: i64, window: &mut Window, cx: &mut App) {
        let _ = self.update(cx, |shell, cx| shell.open(id, window, cx));
    }
    fn open_channels(&self, window: &mut Window, cx: &mut App) {
        let _ = self.update(cx, |shell, cx| shell.open_classic_channels(window, cx));
    }
    fn project_appearance(&self, project: &str, cx: &App) -> Option<ProjectAppearance> {
        self.upgrade().and_then(|shell| {
            let appearance = &shell.read(cx).workspace.project_appearance;
            appearance.get(project).cloned()
        })
    }
    fn projects(&self, cx: &App) -> (Vec<String>, HashMap<String, ProjectAppearance>) {
        self.upgrade()
            .map(|shell| {
                let workspace = &shell.read(cx).workspace;
                (
                    workspace.projects.clone(),
                    workspace.project_appearance.clone(),
                )
            })
            .unwrap_or_default()
    }
}
