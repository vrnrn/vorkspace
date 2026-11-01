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
fn identical_cwd_terminal_pills_keep_hover_and_selection_independent(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, _) = super::super::tests::window(cx);
    let (first, second) = window
        .update(cx, |shell, w, cx| {
            w.activate_window();
            shell.open_terminal_tab(w, cx);
            let first = shell.active_title_tab(cx).unwrap();
            shell.open_terminal_tab(w, cx);
            (first, shell.active_title_tab(cx).unwrap())
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let first_bounds = visual.debug_bounds(title_selector(&first)).unwrap();
    visual.simulate_mouse_move(first_bounds.center(), None, Default::default());
    visual.run_until_parked();
    let key = |tab: &TitleTab| format!("tool:terminal-title-tab-{}", terminal_id(tab));
    window
        .update(visual, |shell, _, _| {
            assert_eq!(
                shell.tab_liquid.selected().as_deref(),
                Some(key(&second).as_str())
            );
            let pills = shell.tab_liquid.pills();
            assert!(pills.contains(&(key(&first).into(), false)), "{pills:?}");
            assert!(pills.contains(&(key(&second).into(), true)));
        })
        .unwrap();
    visual.simulate_click(first_bounds.center(), Default::default());
    visual.run_until_parked();
    let second_bounds = visual.debug_bounds(title_selector(&second)).unwrap();
    visual.simulate_mouse_move(second_bounds.center(), None, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert_eq!(
                shell.tab_liquid.selected().as_deref(),
                Some(key(&first).as_str())
            );
            let pills = shell.tab_liquid.pills();
            assert!(pills.contains(&(key(&first).into(), true)));
            assert!(pills.contains(&(key(&second).into(), false)));
        })
        .unwrap();
}

#[gpui::test]
fn terminal_is_not_a_sidebar_item_and_a_plain_new_tab_click_opens_a_session(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = super::super::tests::window(cx);
    cx.update(|cx| {
        bind_keys(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("sidebar-navigation-Terminal").is_none());
    let options = visual.debug_bounds("sidebar-options").unwrap();
    visual.simulate_click(options.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("menu-action-sidebar-github").is_some());
    assert!(
        visual
            .debug_bounds("menu-action-sidebar-terminal")
            .is_none()
    );
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let tabs = window
        .update(visual, |shell, _, _| shell.workspace.tabs.len())
        .unwrap();
    let plus = visual.debug_bounds("new-tab").unwrap();
    visual.simulate_click(plus.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.tabs.len(), tabs + 1);
            assert!(shell.terminal_tabs.is_empty());
            assert_eq!(shell.active_terminal_tab, None);
        })
        .unwrap();
}

#[gpui::test]
fn command_new_tab_every_click_opens_and_focuses_an_independent_top_tab(cx: &mut TestAppContext) {
    let (window, _, session) = super::super::tests::window(cx);
    cx.update(|cx| {
        bind_keys(cx);
        crate::terminal::bind(cx);
        vorkspace_ui::clock::use_test_clock(cx);
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
            // Command-click on the strip's + opens a terminal tab.
            let button = visual.debug_bounds("new-tab").unwrap();
            visual.simulate_click(button.center(), gpui::Modifiers::command());
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
    let close_selector =
        Box::leak(format!("close-terminal-title-tab-{}", terminal_id(&closed)).into_boxed_str());
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
    visual.simulate_keystrokes("cmd-w");
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
    vorkspace_ui::clock::use_test_clock(cx);
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
                crate::git_tools::worktree_switcher::WorktreeFocus {
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
fn terminal_tab_close_preserves_siblings_and_revalidates_prompt_ownership(cx: &mut TestAppContext) {
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
                0,
                "connector pages are never tabs"
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
            cx.new(|cx| Shell::new(store, Vec::new(), "/work/vorkspace".into(), Some(saved), cx))
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
            assert!(!shell.workspace.title_order.contains(&TitleTab::App(
                app_views::AppView::Connector(Connector::Github)
            )));
        })
        .unwrap();
}
