use super::super::*;
use gpui::TestAppContext;

#[gpui::test]
fn search_field_fills_the_original_project_rail(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let sidebar = visual.debug_bounds("classic-project-rail").unwrap();
    let search = visual.debug_bounds("sidebar-search-field").unwrap();
    assert_eq!(search.left() - sidebar.left(), px(8.));
    assert_eq!(sidebar.right() - search.right(), px(8.));
}

#[gpui::test]
fn sidebar_density_stays_inert_while_activity_visibility_is_restored(cx: &mut TestAppContext) {
    let (window, a, b) = super::super::tests::window(cx);
    let card = window
        .update(cx, |shell, _, cx| {
            // Blank sessions are deliberately absent from navigation. Seed
            // a real history summary so this measures a materialized card.
            let mut summary = locked(&shell.store)
                .unwrap()
                .create("codex", "/work/vorkspace")
                .unwrap();
            summary.has_messages = true;
            summary.title = "Saved conversation".into();
            let card = summary.id;
            shell.session_summaries.insert(card, summary);
            shell.order.insert(0, card);
            for id in [a, b] {
                shell.sessions[&id].update(cx, |view, _| view.busy = true);
            }
            shell.unseen_finished.insert(a);
            shell.workspace.compact_session_cards = false;
            shell.workspace.sidebar_activity_visible = true;
            shell.invalidate_chrome(cx);
            card
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let selector: &'static str = Box::leak(format!("sidebar-session-{card}").into_boxed_str());
    assert_eq!(visual.debug_bounds(selector).unwrap().size.height, px(32.));
    let tree: &'static str = Box::leak(format!("project-session-{b}").into_boxed_str());
    assert!(visual.debug_bounds(tree).is_some());
    window
        .update(visual, |shell, _, cx| {
            let loaded = shell.sessions.len();
            shell.change_sidebar_option("compact-session-cards", cx);
            shell.change_sidebar_option("sidebar-activity", cx);
            assert!(!shell.workspace.compact_session_cards);
            assert!(!shell.workspace.sidebar_activity_visible);
            assert_eq!(shell.sessions.len(), loaded);
            assert!(shell.unseen_finished.contains(&a));
            for id in [a, b] {
                assert!(shell.sessions[&id].read(cx).responding());
            }
            let items = shell.sidebar_options_menu_items(cx);
            assert!(!items.iter().any(
                |item| matches!(item, MenuItem::Action(a) if a.id == "compact-session-cards")
            ));
        })
        .unwrap();
}

#[gpui::test]
fn sidebar_visibility_switches_stay_open_preserve_notes_and_survive_restart(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = super::super::tests::window(cx);
    window
        .update(cx, |shell, w, cx| shell.open_notes(w, cx))
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let subscriptions = visual.debug_bounds("subscriptions").unwrap();
    let settings = visual.debug_bounds("classic-rail-settings").unwrap();
    assert!(subscriptions.bottom() <= settings.top());
    let options = visual.debug_bounds("sidebar-options").unwrap();
    visual.simulate_click(options.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            let menu = shell.menu.as_ref().unwrap().menu.read(cx);
            let bounds = menu.surface_region().get().unwrap();
            assert_eq!(bounds.left(), options.left());
            assert_eq!(bounds.size.width, px(212.));
        })
        .unwrap();
    let notes = visual.debug_bounds("menu-action-sidebar-notes").unwrap();
    let automations = visual
        .debug_bounds("menu-action-sidebar-automations")
        .unwrap();
    assert!(visual.debug_bounds("menu-action-sidebar-inbox").is_none());
    let bots = visual.debug_bounds("menu-action-sidebar-bots").unwrap();
    assert!(
        visual
            .debug_bounds("menu-action-sidebar-activity")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("menu-action-compact-session-cards")
            .is_none()
    );
    assert!(visual.debug_bounds("menu-grip-sidebar-notes").is_some());
    assert!(
        visual
            .debug_bounds("menu-grip-sidebar-automations")
            .is_some()
    );
    assert!(visual.debug_bounds("menu-grip-sidebar-bots").is_none());
    assert!(
        visual
            .debug_bounds("menu-grip-sidebar-subscriptions")
            .is_none()
    );
    assert!(notes.bottom() <= automations.top() && automations.bottom() < bots.top());
    for (action, navigation) in [
        ("menu-action-sidebar-notes", "sidebar-navigation-Notes"),
        (
            "menu-action-sidebar-automations",
            "sidebar-navigation-Automations",
        ),
        ("menu-action-sidebar-bots", "new-bot"),
        ("menu-action-sidebar-subscriptions", "subscriptions"),
    ] {
        assert!(visual.debug_bounds(navigation).is_some());
        let row = visual.debug_bounds(action).unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds(navigation).is_none());
        assert!(
            visual
                .debug_bounds("menu-action-sidebar-subscriptions")
                .is_some()
        );
    }
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(shell.active_app_view, Some(app_views::AppView::Notes));
            assert!(shell.notes.is_some());
            assert!(
                shell.workspace.preferences.notes,
                "hiding the shortcut leaves Notes enabled"
            );
            let menu = shell.menu.as_ref().unwrap().menu.read(cx);
            for id in [
                "sidebar-notes",
                "sidebar-automations",
                "sidebar-bots",
                "sidebar-subscriptions",
            ] {
                assert_eq!(menu.toggle_state(id), Some(false));
            }
            assert!(!cx.global::<vorkspace_ui::preferences::Preferences>().bots);
            let saved = shell.latest_state.lock().unwrap().clone();
            let restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
            assert!(!restored.sidebar_notes_visible);
            assert!(!restored.sidebar_automations_visible);
            assert!(restored.sidebar_inbox_visible);
            assert!(!restored.sidebar_subscriptions_visible);
            assert!(!restored.preferences.bots);
        })
        .unwrap();
    for (action, navigation) in [
        ("menu-action-sidebar-notes", "sidebar-navigation-Notes"),
        (
            "menu-action-sidebar-automations",
            "sidebar-navigation-Automations",
        ),
        ("menu-action-sidebar-bots", "new-bot"),
        ("menu-action-sidebar-subscriptions", "subscriptions"),
    ] {
        let row = visual.debug_bounds(action).unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds(navigation).is_some());
    }
    visual.simulate_click(options.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            shell.open_settings(crate::settings::Section::Bots, w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    let toggle = visual.debug_bounds("settings-toggle-bots-enabled").unwrap();
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            assert!(!shell.workspace.preferences.bots);
            shell.close_settings(w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("new-bot").is_none());
    let options = visual.debug_bounds("sidebar-options").unwrap();
    visual.simulate_click(options.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            let menu = shell.menu.as_ref().unwrap().menu.read(cx);
            assert_eq!(menu.toggle_state("sidebar-bots"), Some(false));
        })
        .unwrap();
}
#[gpui::test]
fn sidebar_grip_drag_reorders_without_toggling_and_preserves_connector_tabs(
    cx: &mut TestAppContext,
) {
    cx.update(crate::popover::bind);
    let (window, _, _) = super::super::tests::window(cx);
    let page = window
        .update(cx, |shell, w, cx| {
            shell.ensure_background_inbox(w, cx);
            shell
                .inbox
                .as_ref()
                .unwrap()
                .update(cx, |owner, _| owner.suspend_refresh_for_test());
            shell.open_connector(Connector::Github, w, cx);
            shell.connector_pages[&Connector::Github].clone()
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let options = visual.debug_bounds("sidebar-options").unwrap();
    visual.simulate_click(options.center(), Default::default());
    visual.run_until_parked();
    for id in ["github", "gitlab", "jira", "notes", "automations"] {
        let selector: &'static str = Box::leak(format!("menu-grip-sidebar-{id}").into_boxed_str());
        assert!(visual.debug_bounds(selector).is_some());
    }
    let grip = visual
        .debug_bounds("menu-grip-sidebar-automations")
        .unwrap();
    // A grip click is not a visibility action.
    visual.simulate_click(grip.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            assert!(shell.workspace.sidebar_automations_visible);
            assert!(
                shell
                    .menu
                    .as_ref()
                    .unwrap()
                    .menu
                    .focus_handle(cx)
                    .is_focused(w)
            );
        })
        .unwrap();
    let end = visual
        .debug_bounds("menu-action-sidebar-github")
        .unwrap()
        .origin
        + gpui::point(px(70.), px(4.));
    visual.simulate_mouse_down(grip.center(), MouseButton::Left, Default::default());
    visual.simulate_mouse_move(
        grip.center() + gpui::point(px(0.), px(-8.)),
        MouseButton::Left,
        Default::default(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            assert_eq!(
                shell.workspace.sidebar_navigation_order,
                ["automations", "github", "gitlab", "jira", "notes", "todos"]
            );
            assert!(shell.workspace.sidebar_automations_visible);
            assert!(shell.workspace.sidebar_hidden_connectors.is_empty());
            let menu = shell.menu.as_ref().unwrap().menu.clone();
            assert!(menu.focus_handle(cx).is_focused(w));
        })
        .unwrap();
    // Escape cancels an in-flight drag, without committing or closing the menu.
    let grip = visual
        .debug_bounds("menu-grip-sidebar-notes")
        .unwrap()
        .center();
    visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(
        grip + gpui::point(px(0.), px(-10.)),
        MouseButton::Left,
        Default::default(),
    );
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert!(shell.menu.is_some());
            assert!(!cx.has_active_drag());
            assert_eq!(shell.workspace.sidebar_navigation_order[0], "automations");
        })
        .unwrap();
    let toggle = visual.debug_bounds("menu-action-sidebar-github").unwrap();
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("sidebar-navigation-GitHub").is_none());
    // Connectors are sidebar pages, not tabs.
    assert!(visual.debug_bounds("app-title-tab-GitHub").is_none());
    window
        .update(visual, |shell, _, cx| {
            assert!(shell.enabled_connectors(cx).contains(&Connector::Github));
            assert_eq!(shell.connector_pages[&Connector::Github], page);
            let saved = shell.latest_state.lock().unwrap().clone();
            let restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
            assert_eq!(
                restored.sidebar_navigation_order,
                shell.workspace.sidebar_navigation_order
            );
            assert!(restored.sidebar_hidden_connectors.contains("github"));
        })
        .unwrap();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    // The same saved projection drives both layouts; Search remains fixed.
    for expanded in [true, false, true] {
        window
            .update(visual, |shell, _, cx| {
                if shell.workspace.project_rail_open != expanded {
                    shell.toggle_project_rail(cx);
                }
                assert_eq!(shell.workspace.project_rail_open, expanded);
            })
            .unwrap();
        visual.run_until_parked();
        let names = [
            "sidebar-navigation-Automations",
            "sidebar-navigation-GitLab",
            "sidebar-navigation-Jira",
            "sidebar-navigation-Notes",
        ];
        let bounds: Vec<_> = names
            .into_iter()
            .map(|id| visual.debug_bounds(id).unwrap())
            .collect();
        assert!(
            bounds
                .windows(2)
                .all(|pair| pair[0].bottom() <= pair[1].top())
        );
        let search = visual
            .debug_bounds(if expanded {
                "sidebar-search-field"
            } else {
                "sidebar-navigation-Search"
            })
            .unwrap();
        assert!(search.bottom() <= bounds[0].top());
    }
    // Disable and re-enable through the existing owner; neither operation
    // discards custom position or the independently hidden shortcut.
    for enabled in [false, true] {
        window
            .update(visual, |shell, _, cx| {
                shell.inbox.as_ref().unwrap().update(cx, |owner, cx| {
                    owner.set_connector_enabled("gitlab", enabled, cx).unwrap()
                });
                shell.invalidate_chrome(cx);
                let items = shell.sidebar_options_menu_items(cx);
                assert_eq!(
                    items.iter().any(
                        |item| matches!(item, MenuItem::Action(a) if a.id == "sidebar-gitlab")
                    ),
                    enabled
                );
                assert_eq!(
                    shell.workspace.sidebar_navigation_order,
                    ["automations", "github", "gitlab", "jira", "notes", "todos"]
                );
                assert!(shell.workspace.sidebar_hidden_connectors.contains("github"));
            })
            .unwrap();
    }
}

#[test]
#[ignore = "Requires Metal and VOKKY_SIDEBAR_CAPTURE_DIR"]
fn sidebar_settings_native_capture() {
    let output = std::path::PathBuf::from(std::env::var_os("VOKKY_SIDEBAR_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let store = Arc::new(Mutex::new(
        vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    let session = locked(&store)
        .unwrap()
        .create("codex", "/work/sidebar-fixture")
        .unwrap();
    let mut state = Workspace::default();
    state.open(session.id);
    state.preferences.tab_animations = false;
    let saved = serde_json::to_string(&state).unwrap();
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
        .open_window(gpui::size(px(1280.), px(800.)), |_, cx| {
            cx.new(|cx| {
                Shell::new(
                    store,
                    vec![session],
                    "/work/sidebar-fixture".into(),
                    Some(saved),
                    cx,
                )
            })
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(&mut cx, |shell, w, cx| {
            shell.open_menu(
                MenuTarget::SidebarOptions,
                Anchor::Slot(shell.sidebar_options_anchor.clone()),
                w,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
    cx.capture_screenshot(window.into())
        .unwrap()
        .save(output.join("sidebar-options.png"))
        .unwrap();
    window
        .update(&mut cx, |shell, _, cx| {
            shell.reorder_sidebar_navigation(
                &crate::popover::MenuReordered {
                    source: "sidebar-automations".into(),
                    target: "sidebar-github".into(),
                    after: false,
                },
                cx,
            );
            shell.change_sidebar_option("sidebar-gitlab", cx);
            shell.close_menu(cx);
        })
        .unwrap();
    for (name, expanded) in [("expanded", true), ("collapsed", false)] {
        window
            .update(&mut cx, |shell, _, cx| {
                if shell.workspace.project_rail_open != expanded {
                    shell.toggle_project_rail(cx);
                }
            })
            .unwrap();
        cx.run_until_parked();
        vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("sidebar-reordered-{name}.png")))
            .unwrap();
    }
    window
        .update(&mut cx, |shell, w, cx| {
            shell.open_settings(crate::settings::Section::Connections, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
    cx.capture_screenshot(window.into())
        .unwrap()
        .save(output.join("settings-app-group.png"))
        .unwrap();
}

#[gpui::test]
fn scroll_and_hover_redraws_reuse_the_sidebar_index(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.refresh_sidebar_structure(cx);
            let rebuilds = shell.sidebar_list.structural_rebuilds;
            for _ in 0..100 {
                // This is the path used by scroll/hover redraws: visible
                // cards refresh, while membership/order stays cached.
                shell.refresh_sidebar_structure(cx);
            }
            assert_eq!(shell.sidebar_list.structural_rebuilds, rebuilds);
            shell.sidebar_list.invalidate_structure();
            shell.refresh_sidebar_structure(cx);
            assert_eq!(shell.sidebar_list.structural_rebuilds, rebuilds + 1);
            // Time expiry is a content refresh, not navigation: retain
            // the user's position instead of snapping back to the active row.
            shell.sidebar_list.state.scroll_to(gpui::ListOffset {
                item_ix: 1,
                offset_in_item: px(5.),
            });
            let offset = shell.sidebar_list.state.logical_scroll_top();
            shell.sidebar_list.indexed_minute = 0;
            shell.refresh_sidebar_structure(cx);
            let refreshed = shell.sidebar_list.state.logical_scroll_top();
            assert_eq!(refreshed.item_ix, offset.item_ix);
            assert_eq!(refreshed.offset_in_item, offset.offset_in_item);
            assert_eq!(shell.sidebar_list.structural_rebuilds, rebuilds + 2);
            // Scope changes are also detected without a notification hook.
            shell.query.push_str("unmatched-session-query");
            shell.refresh_sidebar_structure(cx);
            assert_eq!(shell.sidebar_list.structural_rebuilds, rebuilds + 3);
            assert!(shell.sidebar_list.rows.contains(&super::Row::Empty));
        })
        .unwrap();
}

#[gpui::test]
fn large_project_only_lays_out_nearby_session_cards(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    let started = std::time::Instant::now();
    let (first, last) = window
        .update(cx, |shell, window, cx| {
            let mut summaries = Vec::new();
            {
                let store = locked(&shell.store).unwrap();
                for i in 0..1000 {
                    let mut summary = store.create("codex", "/work/many-sessions").unwrap();
                    summary.title =
                        format!("Session {i}: {}", "A long conversation title 🙂 ".repeat(4));
                    summary.has_messages = true;
                    summaries.push(summary);
                }
            }
            let first = summaries.last().unwrap().id;
            let last = summaries[0].id;
            for summary in summaries {
                shell.order.push(summary.id);
                shell.session_summaries.insert(summary.id, summary);
            }
            shell.workspace.open(first);
            shell.select_project("/work/many-sessions".into(), window, cx);
            (first, last)
        })
        .unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |shell, _, cx| {
            let laid_out = shell
                .session_row_bounds
                .values()
                .filter(|slot| slot.get().is_some())
                .count();
            eprintln!(
                "1,000-session project: {laid_out} laid-out cards, {:?} setup + first frame",
                started.elapsed()
            );
            assert!(laid_out > 0);
            assert!(
                shell.sessions.len() < 10,
                "scrolling the sidebar must not construct off-screen conversations"
            );
            assert!(
                laid_out < 80,
                "off-screen session cards should not be laid out: {laid_out}"
            );
            assert!(shell.session_row_bounds[&first].get().is_some());
            assert!(shell.session_row_bounds[&last].get().is_none());
            shell.sidebar_list.state.scroll_to_end();
            shell.sidebar_frame.update(cx, |_, cx| cx.notify());
        })
        .unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |shell, _, cx| {
            assert!(
                shell.session_row_bounds[&first].get().is_none(),
                "scrolled-away cards cannot remain hit targets"
            );
            assert!(
                shell.session_row_bounds[&last].get().is_some(),
                "the oldest conversation is reachable"
            );
            let members = shell.sidebar_session_ids(cx);
            shell.groups.insert(
                "/work/many-sessions".into(),
                vorkspace_core::session_groups::Groups {
                    folders: vec![vorkspace_core::session_groups::Folder {
                        id: "large-folder".into(),
                        name: "All history".into(),
                        session_ids: members,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            );
            shell.notify_sidebar(cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |shell, _, _| {
            let laid_out = shell
                .session_row_bounds
                .values()
                .filter(|slot| slot.get().is_some())
                .count();
            assert!(
                laid_out > 0 && laid_out < 80,
                "expanded folders must also be virtualized: {laid_out}"
            );
            shell.sidebar_list.state.scroll_to_end();
        })
        .unwrap();
    cx.refresh().unwrap();
    let offset = window
        .update(cx, |shell, window, cx| {
            // Resolve scroll_to_end's end sentinel before checking selection.
            let offset = shell.sidebar_list.state.logical_scroll_top();
            shell.open(first - 1, window, cx);
            offset
        })
        .unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |shell, _, _| {
            // Like the Tauri sidebar, selecting a session (e.g. from a
            // title tab) leaves the user's scroll position alone.
            let after = shell.sidebar_list.state.logical_scroll_top();
            assert_eq!(
                (after.item_ix, after.offset_in_item),
                (offset.item_ix, offset.offset_in_item)
            );
            assert!(shell.session_row_bounds[&(first - 1)].get().is_none());
        })
        .unwrap();
}
