//! The desktop idle policy spares every conversation currently on screen.
use super::*;

impl Shell {
    pub(crate) fn keep_visible_idle_agents(&self, cx: &mut Context<Self>) {
        let mut ids = HashSet::new();
        let mut visible_bots = HashSet::new();
        if self.settings.is_none()
            && let Some(tab) = self.workspace.active_tab()
        {
            for pane in self.workspace.layout(tab).leaf_ids() {
                if let Some(id) = self.workspace.session_for_pane(pane) {
                    ids.insert(id);
                } else if let Some(crate::workspace::AppPane::Bot(bot)) = self
                    .workspace
                    .app_panes
                    .get(&pane)
                    .map(|pane| &pane.content)
                {
                    visible_bots.insert(bot.clone());
                }
            }
        }
        if let Some(preview) = &self.session_preview {
            ids.insert(preview.id);
        }
        if let Some(bots) = &self.bots {
            ids.extend(bots.read(cx).visible_idle_sessions(&visible_bots));
        }
        if let Some(core) = &self.app_core {
            core.update(cx, |core, _| {
                core.keep_visible_agents(self.window_layout, ids)
            });
        }
    }
}
