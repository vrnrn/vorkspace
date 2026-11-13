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
            // It ends at the trigger, kept inside the window, so it spans
            // the trigger and never leaves the screen.
            assert!(bounds.left() >= px(0.));
            assert!(bounds.left() <= options.left() && bounds.right() >= options.right());
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
            .is_none()
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
            assert_eq!(shell.pages.active_app_view, Some(app_views::AppView::Notes));
            assert!(shell.page_tabs.notes.is_some());
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
            let saved = shell.latest_window_layout(cx);
            let restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
            assert!(!restored.sidebar_notes_visible);
            assert!(!restored.sidebar_automations_visible);
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
                .pages
                .inbox
                .as_ref()
                .unwrap()
                .update(cx, |owner, _| owner.suspend_refresh_for_test());
            shell.open_connector(Connector::Github, w, cx);
            shell.pages.connector_pages[&Connector::Github].clone()
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
                [
                    "codebase",
                    "automations",
                    "github",
                    "gitlab",
                    "jira",
                    "notes",
                    "todos"
                ]
            );
            assert!(
                !shell
                    .visible_sidebar_navigation(cx)
                    .contains(&SidebarNavigation::Codebase)
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
            assert_eq!(
                shell.visible_sidebar_navigation(cx).first(),
                Some(&SidebarNavigation::Automations)
            );
            assert_eq!(
                shell.workspace.sidebar_navigation_order,
                [
                    "codebase",
                    "automations",
                    "github",
                    "gitlab",
                    "jira",
                    "notes",
                    "todos"
                ]
            );
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
            assert_eq!(shell.pages.connector_pages[&Connector::Github], page);
            let saved = shell.latest_window_layout(cx);
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
                shell.pages.inbox.as_ref().unwrap().update(cx, |owner, cx| {
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
                    [
                        "codebase",
                        "automations",
                        "github",
                        "gitlab",
                        "jira",
                        "notes",
                        "todos"
                    ]
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
        vorkspace_core::store::Store::test(std::path::Path::new(":memory:")).unwrap(),
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
                Anchor::Slot(shell.sidebar.options_anchor.clone()),
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
            shell.open_settings(crate::settings::Section::General, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
    cx.capture_screenshot(window.into())
        .unwrap()
        .save(output.join("settings-app-group.png"))
        .unwrap();
}
