use super::*;

fn click(visual: &mut gpui::VisualTestContext, selector: &'static str) {
    let point = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("Missing control: {selector}"))
        .center();
    visual.simulate_click(point, Default::default());
}

#[gpui::test]
fn bot_island_collapses_to_face_and_button_and_opens_settings(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    cx.update(|cx| {
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    let s = setup(cx);
    // The island needs the width the project sidebar would otherwise take.
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.sidebar_visible = false;
            cx.notify();
        })
        .unwrap();
    let chat = s.manager.update(cx, |m, cx| {
        m.settings = None;
        let chat = m.chat().unwrap();
        chat.update(cx, |chat, cx| {
            chat.input.update(cx, |input, cx| {
                input.set_text("Keep my Bot draft".into(), cx)
            })
        });
        cx.notify();
        chat
    });
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.simulate_resize(gpui::size(px(1440.), px(900.)));
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(s.window, visual);
    assert!(visual.debug_bounds("bot-island").is_some());
    let island = visual.debug_bounds("bot-island").unwrap();
    s.window
        .update(visual, |_, window, _| {
            let scale = window.scale_factor();
            let fill = window
                .painted_quads()
                .iter()
                .find(|quad| {
                    (quad.bounds.left().0 - island.left().as_f32() * scale).abs() < 0.5
                        && (quad.bounds.size.width.0 - island.size.width.as_f32() * scale).abs()
                            < 0.5
                        && quad.corner_radii.top_left.0 > 0.
                })
                .unwrap()
                .background
                .as_solid()
                .unwrap();
            assert!((fill.a - (vorkspace_ui::theme::pane_fill().a + 0.2).min(1.)).abs() < 0.001);
        })
        .unwrap();
    assert!(visual.debug_bounds("right-panel-frame").is_none());
    assert!(visual.debug_bounds("bot-island-toggle").is_none());
    assert!(visual.debug_bounds("bot-overview-tabs").is_none());

    let normal_composer_width = visual.debug_bounds("composer-box").unwrap().size.width;
    let lane_width = island::ISLAND_BREAKPOINT - COLUMN;
    let chrome_width = 1440.
        - visual
            .debug_bounds("bot-chat-transcript")
            .unwrap()
            .size
            .width
            .as_f32()
        - lane_width;
    for pane_width in [
        island::ISLAND_BREAKPOINT + 1.,
        island::ISLAND_BREAKPOINT,
        island::ISLAND_BREAKPOINT - 1.,
        island::ISLAND_BREAKPOINT,
        island::ISLAND_BREAKPOINT - 1.,
    ] {
        visual.simulate_resize(gpui::size(px(chrome_width + pane_width), px(900.)));
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("bot-island").is_some(),
            pane_width >= island::ISLAND_BREAKPOINT,
            "island visibility at pane width {pane_width}"
        );
        assert_eq!(
            visual.debug_bounds("composer-box").unwrap().size.width,
            normal_composer_width,
            "the island must collapse before compressing the chat column"
        );
    }

    visual.simulate_resize(gpui::size(px(740.), px(800.)));
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-island").is_none());
    assert!(visual.debug_bounds("bot-island-toggle").is_some());
    click(visual, "bot-face-open-island");
    visual.run_until_parked();
    let island = visual.debug_bounds("bot-island").unwrap();
    let pane = visual.debug_bounds("main-pane-frame").unwrap();
    assert!(island.left() > pane.left() && island.right() < pane.right());
    // A Bot with no sessions or documents yet explains both in one line.
    assert!(visual.debug_bounds("bot-island-fresh").is_some());
    assert!(visual.debug_bounds("bot-session-list").is_none());
    click(visual, "bot-island-settings");
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-settings-dialog").is_some());
    click(visual, "bot-settings-Habits");
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-habits-list").is_some());
    assert!(visual.debug_bounds("bot-new-habit").is_some());
    click(visual, "bot-settings-close");
    visual.run_until_parked();
    click(visual, "bot-island-toggle");
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-island").is_some());
    click(visual, "bot-island-close");
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-island").is_none());
    assert_eq!(
        chat.read_with(visual, |chat, cx| chat.input.read(cx).content.clone()),
        "Keep my Bot draft"
    );
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_PAGES_SCREEN_DIR"]
fn native_bot_island_capture() {
    let output = PathBuf::from(std::env::var_os("VORKSPACE_PAGES_SCREEN_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
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
    for (label, width, overlay, section, settings) in [
        ("wide", 1440., false, None, None),
        ("expanded-rail", 1440., false, None, None),
        ("before-collapse", 1200., false, None, None),
        ("after-collapse", 1190., false, None, None),
        (
            "documents",
            1440.,
            false,
            Some(island::Section::Documents),
            None,
        ),
        ("compact", 740., false, None, None),
        ("compact-open", 740., true, None, None),
        (
            "general",
            1100.,
            false,
            None,
            Some(SettingsSection::General),
        ),
    ] {
        let data = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(
            Store::open(&data.path().join("native.sqlite")).unwrap(),
        ));
        let project = data
            .path()
            .join("release-platform")
            .to_string_lossy()
            .to_string();
        let sessions: Vec<_> = (0..18)
            .map(|i| {
                let title = [
                    "Review the release checklist",
                    "Polish the workspace layout",
                    "Investigate the sync issue",
                ][i % 3];
                let mut session = locked(&store).unwrap().create("codex", &project).unwrap();
                session.title = title.into();
                locked(&store).unwrap().title(session.id, title).unwrap();
                session
            })
            .collect();
        let launches = sessions
            .iter()
            .map(|session| bot::Launch {
                session_id: session.id,
                title: session.title.clone(),
                cwd: project.clone(),
                harness: "codex".into(),
                model: None,
                origin_turn: 0,
                notify_on_complete: false,
                completed: false,
                dismissed: false,
            })
            .collect();
        let window = cx
            .open_window(gpui::size(px(width), px(900.)), |_, cx| {
                cx.new(|cx| Shell::new(store, sessions, project.clone(), None, cx))
            })
            .unwrap();
        cx.run_until_parked();
        let manager = window
            .update(&mut cx, |shell, window, cx| {
                shell.workspace.project_rail_open = label == "expanded-rail";
                shell.workspace.preferences.collapsed_project_rail = "compact".into();
                let manager = shell.bot_manager();
                manager.update(cx, |m, cx| m.create_named(vec![project], window, cx));
                manager
            })
            .unwrap();
        cx.run_until_parked();
        manager.update(&mut cx, |m, cx| {
            m.settings = None;
            let id = m.active.clone().unwrap();
            let bot = m.roster.get_mut(&id).unwrap();
            bot.name = "Atlas".into();
            bot.launches = launches;
            bot.documents = (0..18)
                .map(|i| bot::Document {
                    id: format!("doc-{i}"),
                    title: ["Release plan", "Design decisions", "Weekly summary"][i % 3].into(),
                    kind: "document".into(),
                    summary: "Notes and next steps".into(),
                    body: "# Notes\n\nReady for review.".into(),
                    created_at: 0,
                    updated_at: 0,
                })
                .collect();
            if let Some(name) = m.fields.get("bot-name") {
                name.update(cx, |field, cx| field.set_text("Atlas".into(), cx));
            }
            m.configure_chat(&id, cx);
            cx.notify();
        });
        manager.update(&mut cx, |m, cx| {
            if overlay {
                m.open_islands.insert(m.active.clone().unwrap());
            } else {
                m.open_islands.clear();
            }
            if let Some(section) = section {
                m.expanded_islands
                    .insert(m.active.clone().unwrap(), section);
            }
            m.settings = settings;
            cx.notify();
        });
        cx.run_until_parked();
        vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("bot-island-{label}.png")))
            .unwrap();
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}
