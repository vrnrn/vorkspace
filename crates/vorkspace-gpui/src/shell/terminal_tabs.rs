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
        if cwd.starts_with("remote://") {
            self.error = Some("Terminals are not available on remote projects.".into());
            cx.notify();
            return;
        }
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
        self.prepare_app_view(window, cx);
        self.leave_app_views(cx);
        let terminal = self
            .terminal_tabs
            .entry(id)
            .or_insert_with(|| {
                // No Shell observation: grid output belongs to this entity, and
                // the title tab has a stable label. Hidden output cannot wake chat.
                cx.new(|cx| Terminal::new(cwd.clone(), cx))
            })
            .clone();
        terminal.update(cx, |terminal, _| terminal.visible = true);
        self.active_terminal_tab = Some(id);
        self.remember_title_tab(TitleTab::Terminal { id, cwd });
        self.reveal_active_tab(cx);
        self.update_chat_visibility(cx);
        self.sync_right_panel(cx);
        self.invalidate_chrome(cx);
        self.focus_app_view(window, cx);
        self.persist(cx);
        cx.notify();
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
        let terminals: Vec<_> = tabs
            .iter()
            .filter_map(|tab| {
                let TitleTab::Terminal { id, cwd } = tab else {
                    return None;
                };
                let terminal = self.terminal_tabs.get(id);
                Some((
                    *id,
                    cwd.clone(),
                    terminal.map(Entity::downgrade),
                    terminal.and_then(|terminal| terminal.read(cx).foreground_process()),
                ))
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
                            let TitleTab::Terminal { id, .. } = tab else {
                                return true;
                            };
                            terminals
                                .iter()
                                .find(|(expected_id, _, _, _)| expected_id == id)
                                .is_some_and(|(_, _, expected, _)| match expected {
                                    Some(expected) => {
                                        expected.upgrade().as_ref() == this.terminal_tabs.get(id)
                                            && this.terminal_tabs.contains_key(id)
                                    }
                                    None => !this.terminal_tabs.contains_key(id),
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
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn terminal_id(tab: &TitleTab) -> u64 {
        let TitleTab::Terminal { id, .. } = tab else {
            panic!("expected a terminal tab");
        };
        *id
    }

    fn active_terminal(shell: &Shell, cx: &App) -> (TitleTab, Entity<Terminal>) {
        let tab = shell.active_title_tab(cx).unwrap();
        let terminal = shell.terminal_tabs[&terminal_id(&tab)].clone();
        (tab, terminal)
    }

    fn title_selector(tab: &TitleTab) -> &'static str {
        Box::leak(format!("terminal-title-tab-{}", terminal_id(tab)).into_boxed_str())
    }

    #[gpui::test]
    fn terminal_shortcut_reorders_and_hides_without_closing_its_tab(cx: &mut TestAppContext) {
        let (window, _, _) = super::super::tests::window(cx);
        cx.update(|cx| {
            bind_keys(cx);
            cx.set_reduce_motion(true);
        });
        let (tab, terminal) = window
            .update(cx, |shell, w, cx| {
                shell.open_terminal_tab(w, cx);
                active_terminal(shell, cx)
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let options = visual.debug_bounds("sidebar-options").unwrap();
        visual.simulate_click(options.center(), Default::default());
        visual.run_until_parked();
        let grip = visual
            .debug_bounds("menu-grip-sidebar-terminal")
            .unwrap()
            .center();
        let target = visual.debug_bounds("menu-action-sidebar-github").unwrap();
        let end = gpui::point(target.center().x, target.top() + px(3.));
        visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.workspace.sidebar_navigation_order[0], "terminal");
                assert!(shell.workspace.sidebar_terminal_visible);
                assert_eq!(shell.terminal_tabs[&terminal_id(&tab)], terminal);
                let saved = shell.latest_state.lock().unwrap().clone();
                let restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
                assert_eq!(restored.sidebar_navigation_order[0], "terminal");
                assert!(
                    shell
                        .menu
                        .as_ref()
                        .unwrap()
                        .menu
                        .read(cx)
                        .toggle_state("sidebar-terminal")
                        .unwrap()
                );
            })
            .unwrap();
        let row = visual.debug_bounds("menu-action-sidebar-terminal").unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("sidebar-navigation-Terminal").is_none());
        assert!(visual.debug_bounds("terminal-tab-body").is_some());
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.terminal_tabs[&terminal_id(&tab)], terminal);
                assert_eq!(shell.active_terminal_tab, Some(terminal_id(&tab)));
                let saved = shell.latest_state.lock().unwrap().clone();
                assert!(
                    !Workspace::restore(Some(&saved), &[], "/work/vorkspace")
                        .sidebar_terminal_visible
                );
            })
            .unwrap();
        let row = visual.debug_bounds("menu-action-sidebar-terminal").unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        for expanded in [true, false] {
            window
                .update(visual, |shell, _, cx| {
                    shell.workspace.project_rail_open = expanded;
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            let terminal_bounds = visual.debug_bounds("sidebar-navigation-Terminal").unwrap();
            let github_bounds = visual.debug_bounds("sidebar-navigation-GitHub").unwrap();
            assert!(terminal_bounds.bottom() <= github_bounds.top());
        }
        window
            .update(visual, |shell, w, cx| {
                shell.open_connector(Connector::Github, w, cx);
            })
            .unwrap();
        visual.run_until_parked();
        visual.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-w"
        } else {
            "ctrl-w"
        });
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(!shell.workspace.title_order.contains(&TitleTab::App(
                    app_views::AppView::Connector(Connector::Github)
                )));
                assert_eq!(shell.terminal_tabs[&terminal_id(&tab)], terminal);
            })
            .unwrap();
    }

    #[gpui::test]
    fn terminal_sidebar_every_click_opens_and_focuses_an_independent_top_tab(
        cx: &mut TestAppContext,
    ) {
        let (window, _, session) = super::super::tests::window(cx);
        cx.update(|cx| {
            bind_keys(cx);
            crate::terminal::bind(cx);
            cx.set_reduce_motion(true);
        });
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let mut opened: Vec<(TitleTab, Entity<Terminal>)> = Vec::new();
        for expanded in [true, false] {
            window
                .update(visual, |shell, w, cx| {
                    shell.open(session, w, cx);
                    shell.workspace.project_rail_open = expanded;
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            // Exercise the real button repeatedly, including while Terminal is active.
            for _ in 0..2 {
                let button = visual.debug_bounds("sidebar-navigation-Terminal").unwrap();
                visual.simulate_click(button.center(), Default::default());
                visual.run_until_parked();
                assert!(visual.debug_bounds("terminal-tab-body").is_some());
                assert!(visual.debug_bounds("terminal-dock").is_none());
                let (tab, terminal) = window
                    .update(visual, |shell, w, cx| {
                        let (tab, terminal) = active_terminal(shell, cx);
                        for (old_tab, old_terminal) in &opened {
                            assert_ne!(&tab, old_tab);
                            assert_ne!(&terminal, old_terminal);
                            assert!(!old_terminal.read(cx).visible);
                        }
                        assert_eq!(terminal.read(cx).cwd, "/work/vorkspace");
                        assert!(terminal.read(cx).visible);
                        assert!(terminal.focus_handle(cx).is_focused(w));
                        assert_eq!(shell.terminal_tabs.len(), opened.len() + 1);
                        assert_eq!(
                            shell
                                .visible_tabs(cx)
                                .iter()
                                .filter(|tab| matches!(tab, TitleTab::Terminal { .. }))
                                .count(),
                            opened.len() + 1
                        );
                        assert!(shell.terminals.is_empty());
                        terminal.update(cx, |terminal, cx| {
                            terminal.fixture_output(&format!("shell-{}-only", opened.len()), cx)
                        });
                        (tab, terminal)
                    })
                    .unwrap();
                assert!(visual.debug_bounds(title_selector(&tab)).is_some());
                opened.push((tab, terminal));
            }
        }
        // Real title-strip clicks select each exact old shell without admission.
        for (index, (tab, terminal)) in opened.iter().enumerate() {
            let bounds = visual.debug_bounds(title_selector(tab)).unwrap();
            visual.simulate_click(bounds.center(), Default::default());
            visual.run_until_parked();
            window
                .update(visual, |shell, w, cx| {
                    assert_eq!(shell.active_title_tab(cx).as_ref(), Some(tab));
                    assert_eq!(shell.terminal_tabs.len(), opened.len());
                    assert_eq!(shell.terminal_tabs[&terminal_id(tab)], *terminal);
                    assert!(terminal.focus_handle(cx).is_focused(w));
                    let screen = terminal.read(cx).fixture_screen();
                    assert!(screen.contains(&format!("shell-{index}-only")));
                    for other in 0..opened.len() {
                        if other != index {
                            assert!(!screen.contains(&format!("shell-{other}-only")));
                        }
                    }
                })
                .unwrap();
        }
        // The real close button removes only its ID, even though every cwd matches.
        let closed = opened.remove(1).0;
        let close_selector = Box::leak(
            format!("close-terminal-title-tab-{}", terminal_id(&closed)).into_boxed_str(),
        );
        let close = visual.debug_bounds(close_selector).unwrap();
        visual.simulate_click(close.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert!(!shell.workspace.title_order.contains(&closed));
                assert!(!shell.terminal_tabs.contains_key(&terminal_id(&closed)));
                assert_eq!(shell.terminal_tabs.len(), opened.len());
                assert_eq!(
                    shell.active_title_tab(cx).as_ref(),
                    opened.last().map(|(tab, _)| tab)
                );
                for (tab, terminal) in &opened {
                    assert_eq!(shell.terminal_tabs[&terminal_id(tab)], *terminal);
                }
            })
            .unwrap();
        let closed = opened.pop().unwrap().0;
        visual.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-w"
        } else {
            "ctrl-w"
        });
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert!(!shell.workspace.title_order.contains(&closed));
                assert!(!shell.terminal_tabs.contains_key(&terminal_id(&closed)));
                assert_eq!(shell.terminal_tabs.len(), opened.len());
                assert_eq!(
                    shell.active_title_tab(cx).as_ref(),
                    opened.last().map(|(tab, _)| tab)
                );
            })
            .unwrap();
        // The dock shortcut owns a separate entity and keeps every remaining top tab alive.
        visual.simulate_keystrokes("cmd-j");
        visual.run_until_parked();
        window
            .update(visual, |shell, w, cx| {
                assert!(shell.active_terminal_tab.is_none());
                assert_eq!(shell.dock_focused(w, cx), Some(0));
                for (_, terminal) in &opened {
                    assert_ne!(shell.terminals["/work/vorkspace"][0], *terminal);
                }
                assert_eq!(shell.terminal_tabs.len(), opened.len());
            })
            .unwrap();
    }

    #[gpui::test]
    fn terminal_tabs_retain_output_and_working_copy_across_navigation_and_drag(
        cx: &mut TestAppContext,
    ) {
        let (window, _, session) = super::super::tests::window(cx);
        cx.update(|cx| cx.set_reduce_motion(true));
        let (original_tab, terminal) = window
            .update(cx, |shell, w, cx| {
                shell.open_terminal_tab(w, cx);
                let original = active_terminal(shell, cx);
                original.1.update(cx, |terminal, cx| {
                    terminal.fixture_output("retained output", cx)
                });
                shell.open(session, w, cx);
                original
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let before = window
            .update(visual, |shell, _, _| shell.render_passes)
            .unwrap();
        terminal.update(visual, |terminal, cx| {
            assert!(!terminal.visible);
            terminal.fixture_output(" while hidden", cx);
        });
        visual.run_until_parked();
        let feature_tab = window
            .update(visual, |shell, w, cx| {
                assert_eq!(
                    shell.render_passes, before,
                    "hidden output must not repaint Shell/chat"
                );
                shell.worktree_focus.insert(
                    shell.workspace.project.clone(),
                    crate::worktree_switcher::WorktreeFocus {
                        path: "/work/feature".into(),
                        branch: Some("feature".into()),
                    },
                );
                shell.open_terminal_tab(w, cx);
                let (feature, feature_terminal) = active_terminal(shell, cx);
                assert_eq!(feature_terminal.read(cx).cwd, "/work/feature");
                shell.open_terminal_tab(w, cx);
                let (sibling, sibling_terminal) = active_terminal(shell, cx);
                assert_ne!(feature, sibling);
                assert_ne!(feature_terminal, sibling_terminal);
                assert_eq!(sibling_terminal.read(cx).cwd, "/work/feature");
                assert_eq!(shell.terminal_tabs.len(), 3);
                shell.open_title_tab(&original_tab, w, cx);
                assert_eq!(shell.active_title_tab(cx), Some(original_tab.clone()));
                assert_eq!(shell.working_copy(), "/work/feature");
                assert_eq!(terminal.read(cx).cwd, "/work/vorkspace");
                assert!(
                    terminal
                        .read(cx)
                        .fixture_screen()
                        .contains("retained output while hidden")
                );
                assert!(terminal.focus_handle(cx).is_focused(w));
                feature
            })
            .unwrap();
        visual.run_until_parked();
        let original = visual.debug_bounds(title_selector(&original_tab)).unwrap();
        let feature = visual.debug_bounds(title_selector(&feature_tab)).unwrap();
        visual.simulate_mouse_down(original.center(), MouseButton::Left, Default::default());
        visual.simulate_mouse_move(feature.center(), MouseButton::Left, Default::default());
        visual.simulate_mouse_up(feature.center(), MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                let visible = shell.visible_tabs(cx);
                assert!(
                    visible.iter().position(|tab| tab == &original_tab).unwrap()
                        > visible.iter().position(|tab| tab == &feature_tab).unwrap()
                );
                assert_eq!(shell.active_title_tab(cx), Some(original_tab));
            })
            .unwrap();
    }

    #[gpui::test]
    fn terminal_tab_close_preserves_siblings_and_revalidates_prompt_ownership(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = super::super::tests::window(cx);
        let (tab, old, sibling, sibling_entity) = window
            .update(cx, |shell, w, cx| {
                shell.open_terminal_tab(w, cx);
                let (tab, terminal) = active_terminal(shell, cx);
                terminal.update(cx, |terminal, _| {
                    terminal.foreground = Some("fixture-job".into())
                });
                let old = terminal.downgrade();
                shell.open_terminal_tab(w, cx);
                let (sibling, sibling_entity) = active_terminal(shell, cx);
                sibling_entity.update(cx, |terminal, cx| {
                    terminal.fixture_output("sibling output", cx)
                });
                shell.close_title_tabs(std::slice::from_ref(&tab), None, w, cx);
                (tab, old, sibling, sibling_entity)
            })
            .unwrap();
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        let successor = window
            .update(cx, |shell, w, cx| {
                assert_eq!(
                    old.upgrade().as_ref(),
                    shell.terminal_tabs.get(&terminal_id(&tab))
                );
                assert_eq!(shell.active_title_tab(cx), Some(sibling.clone()));
                shell.close_title_tabs(std::slice::from_ref(&tab), None, w, cx);
                // Another explicit close followed by a new admission supersedes the prompt.
                shell.close_title_tabs_confirmed(std::slice::from_ref(&tab), None, w, cx);
                shell.open_terminal_tab(w, cx);
                let successor = shell.active_title_tab(cx).unwrap();
                assert_ne!(successor, tab);
                successor
            })
            .unwrap();
        cx.run_until_parked();
        assert!(
            old.upgrade().is_none(),
            "pending prompt must not retain a closed shell"
        );
        cx.simulate_prompt_answer("Close");
        cx.run_until_parked();
        let replaced = window
            .update(cx, |shell, w, cx| {
                assert_eq!(
                    shell.terminal_tabs.len(),
                    2,
                    "old confirmation must preserve same-cwd shells"
                );
                assert_eq!(shell.active_title_tab(cx), Some(successor.clone()));
                assert_eq!(shell.terminal_tabs[&terminal_id(&sibling)], sibling_entity);
                assert!(
                    sibling_entity
                        .read(cx)
                        .fixture_screen()
                        .contains("sibling output")
                );
                let terminal = &shell.terminal_tabs[&terminal_id(&successor)];
                terminal.update(cx, |terminal, _| {
                    terminal.foreground = Some("replacement-job".into())
                });
                let replaced = terminal.downgrade();
                shell.close_title_tabs(std::slice::from_ref(&successor), None, w, cx);
                // Even restoring the exact saved ID must not let an old entity's prompt close its replacement.
                shell.close_title_tabs_confirmed(std::slice::from_ref(&successor), None, w, cx);
                shell.open_title_tab(&successor, w, cx);
                replaced
            })
            .unwrap();
        cx.run_until_parked();
        assert!(replaced.upgrade().is_none());
        cx.simulate_prompt_answer("Close");
        cx.run_until_parked();
        let final_entity = window
            .update(cx, |shell, w, cx| {
                assert_eq!(shell.terminal_tabs.len(), 2);
                let terminal = &shell.terminal_tabs[&terminal_id(&successor)];
                let final_entity = terminal.downgrade();
                terminal.update(cx, |terminal, _| {
                    terminal.foreground = Some("final-job".into())
                });
                shell.close_title_tabs(std::slice::from_ref(&successor), None, w, cx);
                final_entity
            })
            .unwrap();
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Close");
        cx.run_until_parked();
        assert!(
            final_entity.upgrade().is_none(),
            "explicit close releases the PTY owner"
        );
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.terminal_tabs.len(), 1);
                assert_eq!(shell.active_title_tab(cx), Some(sibling.clone()));
                assert_eq!(shell.terminal_tabs[&terminal_id(&sibling)], sibling_entity);
                assert!(
                    sibling_entity
                        .read(cx)
                        .fixture_screen()
                        .contains("sibling output")
                );
                for closed in [&tab, &successor] {
                    assert!(!shell.workspace.title_order.contains(closed));
                    assert!(
                        !shell
                            .workspace
                            .scope_focus
                            .values()
                            .any(|saved| saved == closed)
                    );
                }
            })
            .unwrap();
    }

    #[gpui::test]
    fn terminal_tabs_restore_duplicate_cwd_identity_order_and_fresh_lazy_shells(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = super::super::tests::window(cx);
        let (saved, store, first, second, old) = window
            .update(cx, |shell, w, cx| {
                shell.open_connector(Connector::Github, w, cx);
                let connector = shell.connector_pages[&Connector::Github].clone();
                shell.open_connector(Connector::Github, w, cx);
                assert_eq!(shell.connector_pages[&Connector::Github], connector);
                assert_eq!(
                    shell
                        .workspace
                        .title_order
                        .iter()
                        .filter(|tab| **tab
                            == TitleTab::App(app_views::AppView::Connector(Connector::Github)))
                        .count(),
                    1
                );
                shell.open_terminal_tab(w, cx);
                let first = shell.active_title_tab(cx).unwrap();
                shell.open_terminal_tab(w, cx);
                let (second, terminal) = active_terminal(shell, cx);
                terminal.update(cx, |terminal, cx| {
                    terminal.fixture_output("old process output", cx)
                });
                shell.reorder_title_tab(&second, 0, cx);
                shell.persist(cx);
                (
                    shell.latest_state.lock().unwrap().clone(),
                    shell.store.clone(),
                    first,
                    second,
                    terminal.entity_id(),
                )
            })
            .unwrap();
        assert!(saved.contains("\"terminal\""));
        let roundtrip = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
        assert_eq!(
            roundtrip
                .title_order
                .iter()
                .filter(|tab| matches!(tab, TitleTab::Terminal { .. }))
                .collect::<Vec<_>>(),
            [&second, &first]
        );
        let restored = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    Shell::new(store, Vec::new(), "/work/vorkspace".into(), Some(saved), cx)
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        restored
            .update(cx, |shell, w, cx| {
                assert_eq!(shell.active_title_tab(cx), Some(second.clone()));
                assert_eq!(shell.workspace.title_order[0], second);
                assert_eq!(
                    shell.terminal_tabs.len(),
                    1,
                    "inactive saved shells restore lazily"
                );
                let fresh = shell.terminal_tabs[&terminal_id(&second)].clone();
                assert_ne!(fresh.entity_id(), old);
                assert_eq!(fresh.read(cx).cwd, "/work/vorkspace");
                assert!(
                    !fresh
                        .read(cx)
                        .fixture_screen()
                        .contains("old process output")
                );
                fresh.update(cx, |terminal, cx| {
                    terminal.fixture_output("restored second only", cx)
                });
                shell.open_title_tab(&first, w, cx);
                let (_, other) = active_terminal(shell, cx);
                assert_eq!(shell.terminal_tabs.len(), 2);
                assert_ne!(fresh, other);
                assert_eq!(other.read(cx).cwd, "/work/vorkspace");
                assert!(
                    !other
                        .read(cx)
                        .fixture_screen()
                        .contains("restored second only")
                );
                shell.open_title_tab(&second, w, cx);
                assert_eq!(shell.terminal_tabs[&terminal_id(&second)], fresh);
                assert!(fresh.focus_handle(cx).is_focused(w));
                shell.open_terminal_tab(w, cx);
                assert_eq!(shell.terminal_tabs.len(), 3);
                let newest = shell.active_title_tab(cx).unwrap();
                assert_ne!(newest, first);
                assert_ne!(newest, second);
                assert!(shell.workspace.title_order.contains(&TitleTab::App(
                    app_views::AppView::Connector(Connector::Github)
                )));
            })
            .unwrap();
    }
}

#[cfg(target_os = "macos")]
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
        vorkspace_core::store::Store::open(&directory.path().join("fixture.sqlite")).unwrap(),
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
    cx.update(|cx| cx.set_reduce_motion(true));
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
    cx.capture_screenshot(window.into())
        .unwrap()
        .save(output)
        .unwrap();
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
}
