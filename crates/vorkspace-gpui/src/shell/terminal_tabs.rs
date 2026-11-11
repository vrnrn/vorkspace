//! Each title-tab identity owns one shell independently of its cwd and the dock.
use super::*;
#[cfg(test)]
use vorkspace_core::files::path_key;

impl Shell {
    /// Every sidebar admission creates a fresh identity. Existing title-tab
    /// selection goes directly to `show_terminal_tab` with that tab's identity.
    pub(super) fn open_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self
            .workspace
            .new_terminal_tab(self.working_copy().to_owned());
        self.open_title_tab(&tab, window, cx);
    }

    pub(super) fn show_terminal_tab(
        &mut self,
        id: u64,
        cwd: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !cfg!(test)
            && !self.terminal_tabs.contains_key(&id)
            && !std::path::Path::new(&cwd).is_dir()
        {
            self.error = Some(format!(
                "Could not open terminal: directory is unavailable: {cwd}"
            ));
            cx.notify();
            return;
        }
        self.prepare_app_view(
            self.app_pane_id(&crate::workspace::AppPane::Terminal {
                id,
                cwd: cwd.clone(),
            }),
            window,
            cx,
        );
        self.leave_app_views(cx);
        let terminal = self
            .terminal_tabs
            .entry(id)
            .or_insert_with(|| {
                // No Shell observation: grid output belongs to this entity, and
                // the title tab has a stable label. Hidden output cannot wake chat.
                cx.new(|cx| Terminal::new(crate::terminal::usable_cwd(cwd.clone()), cx))
            })
            .clone();
        terminal.update(cx, |terminal, cx| terminal.set_visible(true, cx));
        self.active_terminal_tab = Some(id);
        self.admit_app_pane(crate::workspace::AppPane::Terminal { id, cwd });
        self.reveal_active_tab(cx);
        self.update_chat_visibility(cx);
        self.sync_right_panel(cx);
        self.invalidate_chrome(cx);
        self.focus_app_view(window, cx);
        self.persist(cx);
        cx.notify();
    }

    fn terminal_ids_for_tab(&self, tab: &TitleTab) -> Vec<(u64, String)> {
        match tab {
            TitleTab::Terminal { id, cwd } => vec![(*id, cwd.clone())],
            TitleTab::Workspace(id) => self
                .workspace
                .layout(*id)
                .leaf_ids()
                .iter()
                .filter_map(|pane| match &self.workspace.app_panes.get(pane)?.content {
                    crate::workspace::AppPane::Terminal { id, cwd } => Some((*id, cwd.clone())),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
    pub(super) fn confirm_terminal_pane_close(
        &mut self,
        pane: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(crate::workspace::AppPane::Terminal { id, .. }) =
            self.workspace.app_panes.get(&pane).map(|p| &p.content)
        else {
            return false;
        };
        let id = *id;
        let Some(terminal) = self.terminal_tabs.get(&id) else {
            return false;
        };
        let Some(process) = terminal.read(cx).foreground_process() else {
            return false;
        };
        let owner = terminal.downgrade();
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &format!("This terminal is still running {process}. Close it anyway?"),
            None,
            &["Close", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.workspace.tab_of(pane).is_some()
                        && owner.upgrade().as_ref() == this.terminal_tabs.get(&id)
                        && this.terminal_tabs.contains_key(&id)
                    {
                        this.close_pane_confirmed(pane, window, cx);
                    }
                });
            }
        })
        .detach();
        true
    }

    /// Batch closes use the same warning as dock closes. Capture weak entities
    /// so awaiting a prompt neither retains a closed PTY nor closes a successor
    /// created later, including at the same directory or saved tab identity.
    pub(super) fn confirm_terminal_tab_close(
        &mut self,
        tabs: &[TitleTab],
        preferred: Option<TitleTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let snapshots: Vec<_> = tabs
            .iter()
            .map(|tab| (tab.clone(), self.terminal_ids_for_tab(tab)))
            .collect();
        let layouts: Vec<_> = tabs
            .iter()
            .filter_map(|tab| {
                tab.session()
                    .map(|id| (id, self.workspace.layout(id).leaf_ids()))
            })
            .collect();
        let terminals: Vec<_> = snapshots
            .iter()
            .flat_map(|(_, ids)| ids.iter())
            .map(|(id, cwd)| {
                let terminal = self.terminal_tabs.get(id);
                (
                    *id,
                    cwd.clone(),
                    terminal.map(Entity::downgrade),
                    terminal.and_then(|terminal| terminal.read(cx).foreground_process()),
                )
            })
            .collect();
        let running: Vec<_> = terminals
            .iter()
            .filter_map(|(_, cwd, _, process)| {
                process
                    .as_ref()
                    .map(|process| format!("• {} ({process})", project_name(cwd)))
            })
            .collect();
        if running.is_empty() {
            return false;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &format!(
                "These terminals are still running:\n{}\n\nClose them anyway?",
                running.join("\n")
            ),
            None,
            &["Close", "Cancel"],
            cx,
        );
        let tabs = tabs.to_vec();
        cx.spawn_in(window, async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = this.update_in(cx, |this, window, cx| {
                    let closing: Vec<_> = tabs
                        .into_iter()
                        .filter(|tab| {
                            if let Some(id) = tab.session()
                                && !layouts.iter().any(|(original, leaves)| {
                                    *original == id
                                        && *leaves == this.workspace.layout(id).leaf_ids()
                                })
                            {
                                return false;
                            }
                            snapshots
                                .iter()
                                .find(|(original, _)| original == tab)
                                .is_some_and(|(_, ids)| {
                                    ids.iter().all(|(id, _)| {
                                        terminals
                                            .iter()
                                            .find(|(expected_id, _, _, _)| expected_id == id)
                                            .is_some_and(|(_, _, expected, _)| match expected {
                                                Some(expected) => {
                                                    expected.upgrade().as_ref()
                                                        == this.terminal_tabs.get(id)
                                                        && this.terminal_tabs.contains_key(id)
                                                }
                                                None => !this.terminal_tabs.contains_key(id),
                                            })
                                    })
                                })
                        })
                        .collect();
                    this.close_title_tabs_confirmed(&closing, preferred, window, cx);
                });
            }
        })
        .detach();
        true
    }
}

#[cfg(test)]
mod tests;

#[test]
#[ignore = "Requires Metal and VOKKY_TERMINAL_TAB_CAPTURE_PATH"]
fn native_terminal_tab_capture() {
    let output =
        std::path::PathBuf::from(std::env::var_os("VOKKY_TERMINAL_TAB_CAPTURE_PATH").unwrap());
    assert!(output.is_absolute());
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("demo");
    std::fs::create_dir(&project).unwrap();
    let cwd = project.to_string_lossy().into_owned();
    let store = Arc::new(Mutex::new(
        vorkspace_core::store::Store::test(&directory.path().join("fixture.sqlite")).unwrap(),
    ));
    let mut workspace = Workspace::default();
    workspace.add_project(&cwd);
    workspace.project = cwd.clone();
    let tab = workspace.new_terminal_tab(cwd.clone());
    let TitleTab::Terminal { id, .. } = &tab else {
        unreachable!()
    };
    let id = *id;
    workspace.title_order.push(tab.clone());
    workspace
        .scope_focus
        .insert(format!("{}\0{}", path_key(&cwd), path_key(&cwd)), tab);
    let saved = serde_json::to_string(&workspace).unwrap();
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    vorkspace_ui::clock::use_test_clock(&cx);
    let window = cx
        .open_window(gpui::size(px(1200.), px(800.)), |_, cx| {
            cx.new(|cx| Shell::new(store, Vec::new(), cwd.clone(), Some(saved), cx))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |view, _, cx| {
        view.downcast::<Shell>().unwrap().update(cx, |shell, cx| {
            let terminal = &shell.terminal_tabs[&id];
            terminal.update(cx, |terminal, cx| {
                terminal.fixture_output(
                    "demo % printf 'Terminal tab ready\\n'\r\nTerminal tab ready\r\n\r\ndemo % ",
                    cx,
                )
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
    cx.capture_screenshot(window.into())
        .unwrap()
        .save(output)
        .unwrap();
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
}
