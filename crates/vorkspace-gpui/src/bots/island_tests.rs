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
    // A Bot with no changes, sessions or documents yet explains what will appear.
    assert!(visual.debug_bounds("bot-island-title").is_some());
    assert!(visual.debug_bounds("bot-island-unchanged").is_some());
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

#[gpui::test]
fn bot_island_lists_latest_turn_changes_and_recent_launches(cx: &mut TestAppContext) {
    use vorkspace_core::{tool::ToolCall, transcript::Row};
    vorkspace_ui::clock::use_test_clock(cx);
    let s = setup(cx);
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.sidebar_visible = false;
            cx.notify();
        })
        .unwrap();
    let row = |role: &str, text: String| Row {
        role: role.into(),
        text,
        first: true,
        sent_at: None,
    };
    let edit = |id: &str, name: &str, path: &str| {
        row(
            "tool",
            serde_json::to_string(&ToolCall {
                input: json!({ "file_path": path }).to_string(),
                ..ToolCall::new(id, name)
            })
            .unwrap(),
        )
    };
    let now = vorkspace_core::time::now_ms();
    let sessions: Vec<_> = (0..3)
        .map(|_| {
            locked(&s.store)
                .unwrap()
                .create("codex", &s.project)
                .unwrap()
        })
        .collect();
    let transcript = s.manager.update(cx, |m, cx| {
        m.settings = None;
        let bot = m.roster.get_mut(&s.id).unwrap();
        // Unknown age, a day old, and started within the day.
        for (session, started_at) in sessions.iter().zip([
            0,
            now - bot::Launch::LISTED_FOR_MS,
            now - bot::Launch::LISTED_FOR_MS + 60_000,
        ]) {
            bot.launches.push(bot::Launch {
                session_id: session.id,
                title: format!("Launch {}", session.id),
                cwd: s.project.clone(),
                harness: "codex".into(),
                started_at,
                ..Default::default()
            });
        }
        cx.notify();
        m.chat().unwrap().read(cx).transcript.clone()
    });
    transcript.update(cx, |transcript, cx| {
        transcript.replace_history(
            vec![
                row("user", "First request".into()),
                edit("old", "Write", "/work/earlier.rs"),
                row("user", "Second request".into()),
                edit("read", "Read", "/work/only-read.rs"),
                edit("a", "Edit", "/work/src/a.rs"),
                edit("b", "Write", "/work/b.md"),
                edit("a-again", "Edit", "/work/src/a.rs"),
                row("assistant", "Finished".into()),
            ],
            cx,
        );
        assert_eq!(
            transcript.latest_turn_changes(),
            ["/work/src/a.rs", "/work/b.md"]
        );
    });
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.simulate_resize(gpui::size(px(1440.), px(900.)));
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-island-change").is_some());
    assert!(visual.debug_bounds("bot-island-unchanged").is_none());
    assert!(visual.debug_bounds("bot-island-status").is_none());
    s.manager.read_with(visual, |m, _| {
        assert_eq!(m.island_changes[&s.id].1, ["/work/src/a.rs", "/work/b.md"]);
    });
    // Only the launch started within the last day is listed.
    assert!(visual.debug_bounds("bot-island-session").is_some());
    assert!(visual.debug_bounds("bot-island-more-sessions").is_none());

    // The next turn starts with nothing changed.
    transcript.update(visual, |transcript, cx| {
        let mut rows = transcript.response_rows(0).1;
        rows.push(row("user", "Third request".into()));
        transcript.replace_history(rows, cx);
    });
    s.manager.update(visual, |m, cx| {
        let bot = m.roster.get_mut(&s.id).unwrap();
        bot.launches.pop();
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-island-unchanged").is_some());
    assert!(visual.debug_bounds("bot-island-change").is_none());
    assert!(visual.debug_bounds("bot-island-session").is_none());
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
            Store::test(&data.path().join("native.sqlite")).unwrap(),
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
                started_at: vorkspace_core::time::now_ms(),
                ..Default::default()
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
