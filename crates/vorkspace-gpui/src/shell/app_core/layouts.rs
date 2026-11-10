//! Per-window layouts and the focused layout's legacy projection.
use super::*;

impl AppCore {
    pub(in crate::shell) fn allocate_window(&mut self) -> u64 {
        let id = self.next_window_id;
        self.next_window_id += 1;
        id
    }

    fn stage_layouts(&mut self, cx: &mut Context<Self>) {
        self.writes.stage_layouts(self.layouts.clone());
        self.flush_settings(cx);
    }

    fn record_layout(
        &mut self,
        window: u64,
        before: Option<&WindowShared>,
        workspace: &mut Workspace,
        focused: bool,
        durable: bool,
        cx: &mut Context<Self>,
    ) -> Result<Arc<WindowShared>, String> {
        let local = WindowShared::from_workspace(workspace);
        let mut next = self.layouts.clone();
        let shared = next.shared.get_or_insert_with(|| Arc::new(local.clone()));
        if let Some(before) = before.filter(|before| *before != &local) {
            Arc::make_mut(shared).apply_edits(before, &local);
        }
        if shared.as_ref() != &local {
            shared.apply(workspace);
        }
        let shared = shared.clone();
        let shared_changed = self.layouts.shared.as_deref() != Some(shared.as_ref());
        let state = serde_json::to_string(workspace).expect("serialize workspace");
        if !next.record(window, state, focused) && !shared_changed {
            self.set_remote_scope(window, Some(&workspace.active_vorkspace), cx);
            return Ok(shared);
        }
        if durable {
            self.writes.commit_layouts(&self.store, &next)?;
        }
        self.layouts = next;
        if !durable {
            self.stage_layouts(cx);
        }
        self.set_remote_scope(window, Some(&workspace.active_vorkspace), cx);
        if shared_changed {
            self.reconcile_remote_projects(cx);
            cx.notify();
        }
        Ok(shared)
    }

    fn close_layout(&mut self, id: u64, cx: &mut Context<Self>) {
        self.keep_visible_agents(id, HashSet::new());
        self.set_remote_scope(id, None, cx);
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
        self.window_shared = Some(core.update(cx, |core, cx| {
            core.record_layout(
                self.window_layout,
                self.window_shared.as_deref(),
                workspace,
                self.window_active,
                durable,
                cx,
            )
        })?);
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
            .to_string()
    }

    pub(in crate::shell) fn forget_closed_window(&self, cx: &mut Context<Self>) {
        if let Some(core) = self.app_core.clone() {
            core.update(cx, |core, cx| core.close_layout(self.window_layout, cx));
        }
    }
}
