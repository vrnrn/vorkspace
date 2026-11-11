//! Native workspace window layouts. Each record belongs to a window, and
//! the last focused record also feeds the legacy single-window setting.
use super::*;
use gpui::{WindowBounds, point, size};
use serde::{Deserialize, Serialize};

pub(crate) const WINDOW_STATE_KEY: &str = "workspace-windows";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct WindowLayout {
    pub(crate) id: u64,
    pub(crate) workspace: Arc<str>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct WindowLayouts {
    pub(crate) entries: Vec<WindowLayout>,
    pub(crate) focused: Option<u64>,
    pub(super) shared: Option<Arc<WindowShared>>,
    #[serde(skip)]
    pub(super) imported_projects: Vec<String>,
}

impl WindowLayouts {
    pub(crate) fn load(store: &vorkspace_core::store::Store) -> Self {
        Self::load_with_legacy(store, store.ui_state().ok().flatten().as_deref())
    }

    pub(crate) fn load_with_legacy(
        store: &vorkspace_core::store::Store,
        legacy: Option<&str>,
    ) -> Self {
        let mut layouts: Self = store
            .setting(WINDOW_STATE_KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let mut ids = HashSet::new();
        layouts.entries.retain(|entry| {
            entry.id > 0
                && entry.id < u64::MAX / 2
                && serde_json::from_str::<Workspace>(&entry.workspace).is_ok()
                && ids.insert(entry.id)
        });
        if !layouts.focused.is_some_and(|id| ids.contains(&id)) {
            layouts.focused = layouts.entries.first().map(|entry| entry.id);
        }
        if !layouts.entries.is_empty()
            && let Some(current) = legacy
        {
            layouts.reconcile_legacy(current);
        }
        layouts
    }

    fn reconcile_legacy(&mut self, raw: &str) {
        let Ok(legacy) = serde_json::from_str::<Workspace>(raw) else {
            return;
        };
        if let Some(previous) = &self.shared {
            let known = previous
                .projects
                .iter()
                .map(|path| vorkspace_core::files::path_key(path))
                .collect::<HashSet<_>>();
            self.imported_projects = legacy
                .projects
                .iter()
                .filter(|path| !known.contains(&vorkspace_core::files::path_key(path)))
                .cloned()
                .collect();
        }
        self.shared = Some(Arc::new(WindowShared::from_workspace(&legacy)));
        for entry in &mut self.entries {
            // Keep each layout's selected project, panes and zoom. Carry the
            // desktop merge baseline forward so the next launch can detect edits.
            let Ok(mut layout) = serde_json::from_str::<Workspace>(&entry.workspace) else {
                continue;
            };
            layout
                .desktop_project_preferences
                .clone_from(&legacy.desktop_project_preferences);
            layout
                .desktop_preferences_metadata
                .clone_from(&legacy.desktop_preferences_metadata);
            layout
                .desktop_workspace_snapshot
                .clone_from(&legacy.desktop_workspace_snapshot);
            entry.workspace = serde_json::to_string(&layout)
                .expect("serialize reconciled window")
                .into();
        }
    }

    pub(super) fn record(&mut self, id: u64, workspace: String, focused: bool) -> bool {
        let mut changed = false;
        match self.entries.iter_mut().find(|entry| entry.id == id) {
            Some(entry) if entry.workspace.as_ref() != workspace => {
                entry.workspace = workspace.into();
                changed = true;
            }
            Some(_) => {}
            None => {
                self.entries.push(WindowLayout {
                    id,
                    workspace: workspace.into(),
                });
                changed = true;
            }
        }
        if (focused || self.focused.is_none()) && self.focused != Some(id) {
            self.focused = Some(id);
            changed = true;
        }
        changed
    }

    pub(super) fn close(&mut self, id: u64) -> bool {
        if self.entries.len() <= 1 {
            // Closing the last window leaves its layout available to relaunch.
            return false;
        }
        let len = self.entries.len();
        self.entries.retain(|entry| entry.id != id);
        if self.focused == Some(id) {
            self.focused = self.entries.last().map(|entry| entry.id);
        }
        self.entries.len() != len
    }

    pub(super) fn legacy(&self) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| Some(entry.id) == self.focused)
            .map(|entry| entry.workspace.as_ref())
    }

    pub(super) fn settings_values(&self) -> [(&'static str, String); 2] {
        let mut legacy = self
            .legacy()
            .and_then(|raw| serde_json::from_str::<Workspace>(raw).ok())
            .unwrap_or_default();
        if let Some(shared) = &self.shared {
            shared.apply(&mut legacy);
        }
        [
            (
                WINDOW_STATE_KEY,
                serde_json::to_string(self).expect("serialize windows"),
            ),
            (
                "workspace",
                serde_json::to_string(&legacy).expect("serialize the focused workspace"),
            ),
        ]
    }
}

pub(crate) fn additional_window_bounds(
    source: Option<(Bounds<Pixels>, Bounds<Pixels>)>,
    index: usize,
    cx: &App,
) -> WindowBounds {
    let (anchor, display) = source.unwrap_or_else(|| {
        let display = cx
            .primary_display()
            .map(|display| display.visible_bounds())
            .unwrap_or_else(|| Bounds::centered(None, size(px(1180.), px(800.)), cx));
        (display, display)
    });
    let dimensions = size(
        px(1180.).min(display.size.width),
        px(800.).min(display.size.height),
    );
    let shift = px(28. * (index % 8 + 1) as f32);
    let origin = point(
        (anchor.origin.x + shift).clamp(display.left(), display.right() - dimensions.width),
        (anchor.origin.y + shift).clamp(display.top(), display.bottom() - dimensions.height),
    );
    WindowBounds::Windowed(Bounds::new(origin, dimensions))
}

impl Shell {
    pub(crate) fn new_window(
        &mut self,
        vorkspace: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let requested = vorkspace
            .unwrap_or(&self.workspace.active_vorkspace)
            .to_owned();
        let core = self.app_core.clone().expect("registered workspace window");
        let mut state = Workspace {
            preferences: self.workspace.preferences.clone(),
            projects: self.workspace.projects.clone(),
            project_archive: self.workspace.project_archive.clone(),
            deleted_projects: self.workspace.deleted_projects.clone(),
            vorkspaces: self.workspace.vorkspaces.clone(),
            active_vorkspace: requested.clone(),
            vorkspace_project: self.workspace.vorkspace_project.clone(),
            seen_bots: self.workspace.seen_bots.clone(),
            ..Workspace::default()
        };
        if self.workspace.active_vorkspace == requested {
            state.project = self.workspace.project.clone();
        } else {
            state.project = state.vorkspace_landing(&requested);
        }
        let cwd = state.project.clone();
        let saved = serde_json::to_string(&state).expect("serialize new window");
        let source = window
            .display(cx)
            .map(|display| (window.bounds(), display.visible_bounds()));
        let bounds = additional_window_bounds(source, 0, cx);
        let store = self.store.clone();
        let installs = core.read(cx).session_installs.clone();
        let projects = state.projects;
        let mut revision = core.read(cx).catalog_revision;
        cx.spawn(async move |source, cx| {
            let result = loop {
                let read_store = store.clone();
                let projects = projects.clone();
                let installs = installs.clone();
                let work = cx.background_executor().spawn(async move {
                    let db = locked(&read_store)?;
                    let sessions = installs.visible_sessions(&db)?;
                    let metadata = db.session_sidebar_metadata(&projects);
                    Ok::<_, String>((sessions, metadata))
                });
                let (sessions, metadata) = match work.await {
                    Ok(data) => data,
                    Err(error) => break Err(format!("Could not open a window: {error}")),
                };
                let current = cx.update(|cx| core.read(cx).catalog_revision);
                if current != revision {
                    revision = current;
                    continue;
                }
                break cx.update(|cx| {
                    if core
                        .read(cx)
                        .project_deletions
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|path| super::project_delete::within(&cwd, path))
                    {
                        return Err(
                            "That project is being deleted. Try again after it finishes.".into(),
                        );
                    }
                    if !core
                        .read(cx)
                        .vorkspaces
                        .iter()
                        .any(|def| def.id == requested)
                    {
                        return Err("That vorkspace was removed. Choose another one.".into());
                    }
                    core.update(cx, |core, cx| {
                        core.reconcile_session_directory(&sessions, &metadata, true, cx);
                    });
                    crate::open_workspace_window(
                        store,
                        &sessions,
                        &metadata,
                        crate::WorkspaceWindowStart {
                            cwd,
                            saved: Some(saved),
                            layout: None,
                            bounds,
                        },
                        cx,
                    )
                    .map_err(|error| format!("Could not open a window: {error}"))?;
                    cx.activate(true);
                    Ok(())
                });
            };
            if let Err(error) = result {
                let _ = source.update(cx, |shell, cx| {
                    shell.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn request_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.allow_window_close(window, cx) {
            window.remove_window();
        }
    }

    pub(super) fn allow_window_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        #[cfg(not(test))]
        if cx
            .windows()
            .iter()
            .filter(|handle| handle.downcast::<Shell>().is_some())
            .count()
            == 1
            && self.workspace.preferences.close_to_tray
            && crate::background_helper::menu_bar_ready(cx)
            && !self.unsaved_files(cx)
        {
            vorkspace_ui::traffic_lights::hide(window);
            return false;
        }
        if self.unsaved_files(cx) {
            if !self.quit_prompted {
                self.quit_prompted = true;
                let answer = window.prompt(
                    gpui::PromptLevel::Warning,
                    "Close window with unsaved file edits?",
                    Some("Save the files before closing to keep their changes."),
                    &["Close Without Saving", "Cancel"],
                    cx,
                );
                cx.spawn_in(window, async move |shell, cx| {
                    let answer = answer.await;
                    let _ = shell.update_in(cx, |shell, window, cx| {
                        shell.quit_prompted = false;
                        if matches!(answer, Ok(0)) && shell.finish_window_close(cx) {
                            window.remove_window();
                        }
                    });
                })
                .detach();
            }
            return false;
        }
        self.finish_window_close(cx)
    }

    fn finish_window_close(&mut self, cx: &mut Context<Self>) -> bool {
        let conversation_active = self.sessions.values().any(|view| {
            let view = view.read(cx);
            view.external_submission_pending()
        });
        let bot_active = self
            .bots
            .as_ref()
            .is_some_and(|bots| bots.read(cx).has_active_work(cx));
        let terminal_active = self
            .terminal_tabs
            .values()
            .chain(self.terminals.values().flatten())
            .any(|terminal| terminal.read(cx).foreground_process().is_some());
        let voice_active = self
            .vox
            .as_ref()
            .is_some_and(|voice| voice.read(cx).running());
        if conversation_active
            || bot_active
            || terminal_active
            || voice_active
            || self.automation_work_in_progress()
            || self
                .group_writes
                .as_ref()
                .is_some_and(|task| !task.is_ready())
            || !self.archiving.is_empty()
            || !self.deleting.is_empty()
            || !self.deleting_projects.is_empty()
            || self.active_worktree_removals > 0
            || !self.pending_project_creations.is_empty()
            || self
                .preparing_sessions
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0
            || self.pending_last_window_bot_writes(cx)
            || self.quick_window_active(cx)
        {
            self.error =
                Some("This window has work in progress. Finish or stop it before closing.".into());
            cx.notify();
            return false;
        }
        if !self.prepare_notes_close(cx) {
            self.error = Some("A note in this window still has unsaved changes.".into());
            cx.notify();
            return false;
        }
        for view in self.sessions.values() {
            view.update(cx, |view, cx| view.save_side_question_draft(cx));
        }
        self.window_closing = true;
        self.forget_closed_window(cx);
        true
    }
}

/// App-wide choices shared by the window layouts. UI zoom follows the window.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct WindowShared {
    pub(super) preferences: vorkspace_ui::preferences::Preferences,
    projects: Vec<String>,
    archived: HashMap<String, bool>,
    deleted: HashSet<String>,
}

impl WindowShared {
    pub(super) fn from_workspace(workspace: &Workspace) -> Self {
        let mut preferences = workspace.preferences.clone();
        preferences.ui_scale = 100;
        Self {
            preferences,
            projects: workspace.projects.clone(),
            archived: workspace.project_archive.clone(),
            deleted: workspace.deleted_projects.clone(),
        }
    }

    pub(super) fn apply(&self, workspace: &mut Workspace) {
        let zoom = workspace.preferences.ui_scale;
        workspace.preferences.clone_from(&self.preferences);
        workspace.preferences.ui_scale = zoom;
        workspace.projects.clone_from(&self.projects);
        workspace.project_archive.clone_from(&self.archived);
        workspace.deleted_projects.clone_from(&self.deleted);
    }

    pub(super) fn apply_edits(&mut self, before: &Self, next: &Self) {
        self.preferences
            .apply_edits(&before.preferences, &next.preferences);
        if next.projects != before.projects {
            self.projects
                .retain(|path| !before.projects.contains(path) || next.projects.contains(path));
            for path in &next.projects {
                if !before.projects.contains(path) && !self.projects.contains(path) {
                    self.projects.push(path.clone());
                }
            }
        }
        for (key, &archived) in &next.archived {
            if before.archived.get(key) != Some(&archived) {
                self.archived.insert(key.clone(), archived);
            }
        }
        self.deleted
            .retain(|key| !before.deleted.contains(key) || next.deleted.contains(key));
        self.deleted
            .extend(next.deleted.difference(&before.deleted).cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AnyWindowHandle, TestAppContext, WindowHandle};

    fn workspace_windows(cx: &mut TestAppContext) -> Vec<WindowHandle<Shell>> {
        cx.update(|cx| {
            cx.windows()
                .into_iter()
                .filter_map(|handle| handle.downcast::<Shell>())
                .collect()
        })
    }

    #[gpui::test]
    fn native_window_commands_keep_and_restore_separate_layouts(cx: &mut TestAppContext) {
        let (first, _, _) = crate::shell::tests::window(cx);
        cx.update(crate::app_menu::init);
        let (side, main, store, cwd) = first
            .update(cx, |shell, window, cx| {
                let side = shell.workspace.create_vorkspace("Side projects");
                shell.persist(cx);
                window.activate_window();
                (
                    side,
                    shell.workspace.active_vorkspace.clone(),
                    shell.store.clone(),
                    shell.workspace.project.clone(),
                )
            })
            .unwrap();

        cx.simulate_keystrokes(first.into(), "cmd-shift-n");
        cx.run_until_parked();
        let first_handle: AnyWindowHandle = first.into();
        let second = workspace_windows(cx)
            .into_iter()
            .find(|window| AnyWindowHandle::from(*window) != first_handle)
            .expect("File shortcut opens a workspace window");
        assert_eq!(workspace_windows(cx).len(), 2);

        let peer_scale = std::rc::Rc::new(std::cell::Cell::new(None));
        let observed = peer_scale.clone();
        first
            .update(cx, |shell, window, cx| {
                shell.scale_interface(30, window, cx);
                shell.workspace.preferences.theme = "light".into();
                shell.persist(cx);
                // Observe before GPUI can render the peer and correct a
                // scale inadvertently copied from the global preferences.
                cx.defer(move |cx| {
                    second
                        .update(cx, |_, window, _| observed.set(Some(window.ui_scale())))
                        .unwrap();
                });
            })
            .unwrap();
        assert_eq!(peer_scale.get(), Some(1.));
        let second_handle: AnyWindowHandle = second.into();
        cx.simulate_window_resize(second_handle, size(px(860.), px(565.)));
        cx.run_until_parked();
        second
            .update(cx, |shell, window, cx| {
                assert_eq!(shell.workspace.active_vorkspace, main);
                assert_eq!(shell.workspace.preferences.ui_scale, 100);
                assert_eq!(window.ui_scale(), 1.);
                assert_eq!(shell.workspace.preferences.theme, "light");
                shell.run_vorkspace_menu(&format!("new-window:{side}"), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let third = workspace_windows(cx)
            .into_iter()
            .find(|window| {
                let handle: AnyWindowHandle = (*window).into();
                handle != first_handle && handle != second_handle
            })
            .expect("Vorkspace switcher opens the selected vorkspace");
        let third_id = third
            .update(cx, |shell, window, cx| {
                assert_eq!(shell.workspace.active_vorkspace, side);
                shell.scale_interface(-15, window, cx);
                shell.window_active = true;
                shell.persist(cx);
                shell.window_layout
            })
            .unwrap();

        second
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.simulate_keystrokes(second.into(), "cmd-shift-w");
        cx.run_until_parked();
        assert_eq!(workspace_windows(cx).len(), 2);
        assert!(second.update(cx, |_, _, _| ()).is_err());

        let (layouts, saved, summaries, metadata) = {
            let db = locked(&store).unwrap();
            let layouts = WindowLayouts::load(&db);
            assert_eq!(layouts.entries.len(), 2);
            assert_eq!(layouts.focused, Some(third_id));
            (
                layouts,
                db.ui_state().unwrap(),
                db.sessions().unwrap(),
                db.session_sidebar_metadata(&[]),
            )
        };
        // Simulate app teardown. Closing individual windows through the user
        // command would intentionally remove them from this restore set.
        for window in workspace_windows(cx) {
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        }
        cx.run_until_parked();
        cx.update(|cx| {
            crate::restore_workspace_windows(
                store,
                summaries,
                metadata,
                layouts,
                crate::WorkspaceWindowStart {
                    cwd,
                    saved,
                    layout: None,
                    bounds: crate::launch_bounds(cx),
                },
                false,
                cx,
            )
            .unwrap();
        });
        cx.run_until_parked();

        let mut restored = workspace_windows(cx)
            .into_iter()
            .map(|window| {
                window
                    .update(cx, |shell, window, _| {
                        assert_eq!(
                            window.ui_scale(),
                            shell.workspace.preferences.interface_scale()
                        );
                        (
                            shell.workspace.active_vorkspace.clone(),
                            shell.workspace.preferences.ui_scale,
                        )
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();
        restored.sort();
        let mut expected = vec![(main, 130), (side, 85)];
        expected.sort();
        assert_eq!(restored, expected);
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn desktop_import_refreshes_shared_defaults_without_replacing_local_layout() {
        let path = std::path::Path::new(":memory:");
        let store = vorkspace_core::store::Store::open(path).unwrap();
        let mut local = Workspace {
            project: "/workspace/first".into(),
            projects: vec!["/workspace/first".into()],
            right_panel_open: true,
            ..Workspace::default()
        };
        local.preferences.ui_scale = 135;
        local.preferences.default_provider = Some("codex".into());
        let layouts = WindowLayouts {
            entries: vec![WindowLayout {
                id: 1,
                workspace: serde_json::to_string(&local).unwrap().into(),
            }],
            focused: Some(1),
            shared: Some(Arc::new(WindowShared::from_workspace(&local))),
            ..Default::default()
        };
        let mut updated = local.clone();
        updated
            .projects
            .push("/workspace/new-desktop-project".into());
        updated.preferences.default_provider = Some("claude".into());
        updated.desktop_project_preferences =
            Some(serde_json::json!({"default_provider":"claude"}));
        updated.preferences.ui_scale = 100;
        store
            .save_settings(&[
                (WINDOW_STATE_KEY, &serde_json::to_string(&layouts).unwrap()),
                ("workspace", &serde_json::to_string(&updated).unwrap()),
            ])
            .unwrap();
        let loaded = WindowLayouts::load(&store);
        let mut restored: Workspace = serde_json::from_str(&loaded.entries[0].workspace).unwrap();
        loaded.shared.unwrap().apply(&mut restored);
        assert_eq!(
            restored.preferences.default_provider.as_deref(),
            Some("claude")
        );
        assert_eq!(restored.preferences.ui_scale, 135);
        assert_eq!(restored.project, local.project);
        assert!(restored.right_panel_open);
        assert_eq!(
            restored.desktop_project_preferences,
            updated.desktop_project_preferences
        );
        assert_eq!(loaded.imported_projects, ["/workspace/new-desktop-project"]);
    }
}
