use super::*;

impl Shell {
    pub(super) fn configure_quick_composer(&mut self, cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            core.update(cx, |core, cx| {
                if let Some(receiver) = core.quick_shortcut.configure(&self.workspace.preferences) {
                    core.quick_shortcut.task = Some(cx.spawn(async move |_, cx| {
                        while receiver.recv().await.is_ok() {
                            cx.update(|cx| {
                                if let Some(target) = workspace_target(cx)
                                    .and_then(|window| window.downcast::<Shell>())
                                {
                                    let _ = target.update(cx, Shell::toggle_quick_composer);
                                }
                            });
                        }
                    }));
                }
            });
        }
    }

    pub(super) fn quick_window_active(&self, cx: &App) -> bool {
        self.quick
            .window
            .is_some_and(|handle| cx.windows().contains(&handle.into()))
    }

    pub(super) fn toggle_quick_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.management_modal_focus(cx).is_some() {
            return;
        }
        self.quick.window = None;
        let existing = cx
            .windows()
            .into_iter()
            .find_map(|handle| handle.downcast::<crate::composer::quick_composer::QuickComposer>());
        if let Some(handle) = existing
            && handle
                .update(cx, |_, window, _| window.remove_window())
                .is_ok()
        {
            return;
        }
        let cwd = self.workspace.project.clone();
        let kind = self.default_kind();
        let p = &self.workspace.preferences;
        let initial = crate::composer::quick_composer::Launch {
            cwd: cwd.clone(),
            kind,
            model: p.default_models.get(kind.id()).cloned(),
            access: p
                .default_access
                .as_deref()
                .map(vorkspace_core::agent::AccessMode::from_id)
                .unwrap_or_default(),
            prompt: String::new(),
            foreground: false,
            browsed_folder: false,
        };
        let projects = self.workspace.vorkspace_projects();
        let mut choice = self
            .quick
            .last
            .clone()
            .filter(|v| self.workspace.vorkspace_has_project(&v.cwd))
            .unwrap_or(initial);
        choice.access = crate::conversation::default_access(cx);
        let store = self.store.clone();
        let scale = self.workspace.preferences.interface_scale();
        let bounds = gpui::Bounds::centered(None, gpui::size(px(680.), px(230.)), cx);
        let handle = cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                kind: gpui::WindowKind::PopUp,
                titlebar: None,
                is_resizable: false,
                app_id: Some("com.vorkspace.gpui.quick-composer".into()),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| {
                    crate::composer::quick_composer::QuickComposer::new(projects, choice, store, cx)
                });
                view.update(cx, |v, cx| {
                    window.set_ui_scale(scale, cx);
                    v.focus(window, cx);
                });
                view
            },
        );
        match handle {
            Ok(handle) => {
                let view = handle.update(cx, |_, _, cx| cx.entity()).unwrap();
                let vorkspace = self.workspace.active_vorkspace.clone();
                self.quick.events = Some(cx.subscribe_in(
                    &view,
                    window,
                    move |this, view, launch: &crate::composer::quick_composer::Launch, window, cx| {
                        if this.workspace.active_vorkspace != vorkspace {
                            view.update(cx, |view, cx| view.show_error("The active vorkspace changed. Reopen Quick Composer to choose a project there.", cx));
                            return;
                        }
                        if this.deleting_projects.contains(&vorkspace_core::files::path_key(&launch.cwd)) {
                            view.update(cx, |view, cx| view.show_error("This project is being deleted.", cx));
                            return;
                        }
                        if !this.workspace.vorkspace_has_project(&launch.cwd) {
                            if launch.browsed_folder {
                                this.workspace.add_project_to_vorkspace(&vorkspace, &launch.cwd);
                                this.persist(cx);
                            } else {
                                view.update(cx, |view, cx| view.show_error("Choose a project in the current vorkspace.", cx));
                                return;
                            }
                        }
                        this.quick.last = Some(launch.clone());
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                        this.quick.window = None;
                        this.launch_quick_composer(launch.clone(), window, cx);
                    },
                ));
                self.quick.window = Some(handle);
            }
            Err(e) => self.quick.error = Some(e.to_string()),
        }
    }
    fn launch_quick_composer(
        &mut self,
        launch: crate::composer::quick_composer::Launch,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace.vorkspace_has_project(&launch.cwd) {
            self.error = Some("Choose a project in the current vorkspace.".into());
            cx.notify();
            return;
        }
        let key = vorkspace_core::files::path_key(&launch.cwd);
        if self.deleting_projects.contains(&key) {
            self.error = Some("This project is being deleted.".into());
            cx.notify();
            return;
        }
        *self
            .pending_project_creations
            .entry(key.clone())
            .or_default() += 1;
        let store = self.store.clone();
        let choice = launch.clone();
        let navigation_revision = self.navigation_revision;
        crate::conversation::remember_access(choice.access, cx);
        let effort = crate::conversation::preferred_effort(choice.kind.id(), cx);
        let account = crate::accounts::get(cx)
            .map(|a| a.read(cx).selected(launch.kind, &launch.cwd))
            .unwrap_or_default();
        let mut install = self.session_install(cx);
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            let summary = store.create(choice.kind.id(), &choice.cwd)?;
            install.track(summary.id);
            let settings = vorkspace_core::agent::Settings {
                account,
                model: choice.model,
                access: choice.access,
                effort,
                ..Default::default()
            };
            store.save_agent_settings(summary.id, &settings)?;
            Ok::<_, String>((summary, settings, install))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_project_creation(&key);
                match result {
                    Ok((summary, settings, mut install)) => {
                        let id = summary.id;
                        let Some(view) = this.install_prepared(summary, &mut install, cx) else {
                            this.session_install_failed(cx);
                            return;
                        };
                        view.update(cx, |v, cx| v.quick_submit(launch.prompt, settings, cx));
                        this.publish_prepared(&view, cx);
                        this.order.retain(|i| *i != id);
                        this.order.insert(0, id);
                        if launch.foreground
                            && this.navigation_revision == navigation_revision
                            && this.workspace.vorkspace_has_project(&launch.cwd)
                        {
                            this.close_settings(window, cx);
                            this.open(id, window, cx);
                            window.activate_window();
                        }
                        this.invalidate_chrome(cx);
                        this.persist(cx);
                        cx.notify();
                    }
                    Err(e) => {
                        this.error = Some(e);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod scale_tests {
    use gpui::TestAppContext;
    #[gpui::test]
    fn quick_composer_inherits_and_tracks_the_workspace_scale(cx: &mut TestAppContext) {
        let (main, _, _) = crate::shell::tests::window(cx);
        let quick = main
            .update(cx, |shell, window, cx| {
                shell.initialize_scale(window, cx);
                shell.workspace.preferences.ui_scale = 150;
                shell.apply_preferences(cx);
                shell.toggle_quick_composer(window, cx);
                shell.quick.window.unwrap()
            })
            .unwrap();
        cx.run_until_parked();
        for (scale, expected) in [(150, 150), (75, 75), (125, 125), (200, 160), (50, 60)] {
            main.update(cx, |shell, _, cx| {
                shell.workspace.preferences.ui_scale = scale;
                shell.apply_preferences(cx);
            })
            .unwrap();
            cx.run_until_parked();
            let handles: [gpui::AnyWindowHandle; 2] = [main.into(), quick.into()];
            for handle in handles {
                let actual = handle.update(cx, |_, window, _| window.ui_scale()).unwrap();
                assert_eq!(actual, expected as f32 / 100.);
            }
        }
        quick
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        main.update(cx, |shell, _, cx| {
            shell.workspace.preferences.ui_scale = 100;
            shell.apply_preferences(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
}
