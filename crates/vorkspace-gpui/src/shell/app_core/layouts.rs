//! Per-window layouts and the focused layout's legacy projection.
use super::*;

use crate::shell::windows::WINDOW_STATE_KEY;

impl AppCore {
    pub(in crate::shell) fn allocate_window(&mut self) -> u64 {
        let id = self.next_window_id;
        self.next_window_id += 1;
        id
    }

    fn layout_values(layouts: &WindowLayouts) -> (String, String) {
        let mut legacy: Workspace = layouts
            .legacy()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default();
        if let Some(shared) = &layouts.shared {
            shared.apply(&mut legacy);
        }
        (
            serde_json::to_string(layouts).expect("serialize windows"),
            serde_json::to_string(&legacy).expect("serialize the focused workspace"),
        )
    }

    fn stage_layouts(&mut self, cx: &mut Context<Self>) {
        let (windows, legacy) = Self::layout_values(&self.layouts);
        self.stage_settings([(WINDOW_STATE_KEY, windows), ("workspace", legacy)], cx);
    }

    fn record_layout(
        &mut self,
        window: u64,
        before: Option<&WindowShared>,
        workspace: &mut Workspace,
        focused: bool,
        durable: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let local = WindowShared::from_workspace(workspace);
        let mut next = self.layouts.clone();
        let shared = next.shared.get_or_insert_with(|| local.clone());
        if let Some(before) = before.filter(|before| *before != &local) {
            shared.apply_edits(before, local);
        }
        shared.apply(workspace);
        let shared_changed = next.shared != self.layouts.shared;
        let state = serde_json::to_string(workspace).expect("serialize workspace");
        if !next.record(window, state, focused) && !shared_changed {
            return Ok(());
        }
        if durable {
            let (windows, legacy) = Self::layout_values(&next);
            self.writes.commit(
                &self.store,
                &[(WINDOW_STATE_KEY, &windows), ("workspace", &legacy)],
            )?;
        }
        self.layouts = next;
        if !durable {
            self.stage_layouts(cx);
        }
        if shared_changed {
            cx.notify();
        }
        Ok(())
    }

    fn close_layout(&mut self, id: u64, cx: &mut Context<Self>) {
        self.keep_visible_agents(id, HashSet::new());
        if self.dock_badges.remove(&id).is_some() {
            crate::notifications::platform::set_dock_badge(self.dock_badges.values().sum());
        }
        if self.layouts.close(id) {
            self.stage_layouts(cx);
        }
    }
}

impl Shell {
    pub(in crate::shell) fn take_shared_window_state(
        &mut self,
        core: &Entity<AppCore>,
        cx: &mut Context<Self>,
    ) -> bool {
        let shared = core.read(cx).layouts.shared.clone();
        if shared == self.window_shared {
            return false;
        }
        let Some(shared) = shared else {
            return false;
        };
        let preferences = self.workspace.preferences.clone();
        shared.apply(&mut self.workspace);
        self.window_shared = Some(shared);
        if preferences != self.workspace.preferences {
            self.apply_preferences(cx);
            self.sync_settings(cx);
        }
        true
    }

    pub(in crate::shell) fn save_window_state(
        &mut self,
        mut next: Option<Workspace>,
        durable: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.window_closing {
            return Ok(());
        }
        let core = self.app_core.clone().expect("registered workspace window");
        let workspace = next.as_mut().unwrap_or(&mut self.workspace);
        core.update(cx, |core, cx| {
            core.record_layout(
                self.window_layout,
                self.window_shared.as_ref(),
                workspace,
                self.window_active,
                durable,
                cx,
            )
        })?;
        self.window_shared = Some(WindowShared::from_workspace(workspace));
        if let Some(next) = next {
            self.workspace = next;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::shell) fn latest_window_layout(&self, cx: &App) -> String {
        self.app_core
            .as_ref()
            .unwrap()
            .read(cx)
            .layouts
            .entries
            .iter()
            .find(|entry| entry.id == self.window_layout)
            .expect("persisted window layout")
            .workspace
            .clone()
    }

    pub(in crate::shell) fn forget_closed_window(&self, cx: &mut Context<Self>) {
        if let Some(core) = self.app_core.clone() {
            core.update(cx, |core, cx| core.close_layout(self.window_layout, cx));
        }
    }
}
