use super::*;
use gpui::{
    Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions, point,
    size,
};

fn window(section: Section, cx: &mut TestAppContext) -> WindowHandle<SettingsView> {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1200.), px(800.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut view = SettingsView::new(
                        section,
                        Preferences::default(),
                        SettingsContext::default(),
                        cx,
                    );
                    view.discovery_task = None;
                    view.discovery_loading = false;
                    view
                })
            },
        )
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
}

#[gpui::test]
fn skills_document_has_its_own_full_height_pane_and_compact_actions(cx: &mut TestAppContext) {
    let window = window(Section::Skills, cx);
    window
        .update(cx, |view, _, cx| {
            let path = std::path::PathBuf::from("/project/.agents/skills/review/SKILL.md");
            view.discovery.skills = vec![data::Skill {
                name: "review".into(),
                description: "Review changes".into(),
                path: path.clone(),
                scope: "project".into(),
                provider: "agents".into(),
            }];
            view.skill_preview = Some(path);
            view.skill_preview_text = Some(Ok(skill_document::DocumentData::new(format!(
                "---\nname: review\n---\n# Review\n\n{}",
                "Instructions.\n\n".repeat(150)
            ))));
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let left = visual.debug_bounds("skills-list-scroll").unwrap();
    let right = visual.debug_bounds("skill-preview-pane").unwrap();
    let body = visual.debug_bounds("skill-document").unwrap();
    assert_eq!(left.size.height, px(800.));
    assert_eq!(right.size.height, px(800.));
    assert!(right.left() >= left.right());
    assert!(body.bottom() <= right.bottom());
    assert!(body.size.height > px(600.));
    assert!(visual.debug_bounds("transcript").is_none());
    for id in [
        "skill-preview-action",
        "skill-copy-action",
        "skill-reveal-action",
    ] {
        assert_eq!(
            visual.debug_bounds(id).unwrap().size,
            size(px(20.), px(20.))
        );
    }
    let source = visual
        .debug_bounds("settings-skill-document-source")
        .unwrap();
    assert!(source.size.width < px(100.));
    visual.simulate_click(source.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.prefs.skill_source_paths.len(), 1)
        })
        .unwrap();
    visual.simulate_resize(size(px(700.), px(800.)));
    visual.run_until_parked();
    // Deliver the frame that applies the measured Skills container breakpoint.
    visual.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    visual.run_until_parked();
    let left = visual.debug_bounds("skills-list-scroll").unwrap();
    let right = visual.debug_bounds("skill-preview-pane").unwrap();
    assert!(
        right.top() >= left.bottom(),
        "list={left:?}, preview={right:?}"
    );
    assert_eq!(left.size.width, px(700.));
    assert_eq!(right.size.width, px(700.));
    assert!(visual.debug_bounds("skill-document").unwrap().size.height > px(200.));
}

#[gpui::test]
fn bot_mascot_is_beside_name_and_sidebar_switch_stays_visible(cx: &mut TestAppContext) {
    let window = window(Section::Bots, cx);
    window
        .update(cx, |view, _, cx| {
            view.ctx.bots = vec![BotSetting {
                id: "layout-bot".into(),
                name: "Big Boss".into(),
                mascot: "ghost".into(),
                color: 0xee99bb,
                projects: vec!["vorkspace".into(), "bot-web".into()],
                show: false,
                folders: false,
            }];
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let mascot = visual.debug_bounds("settings-bot-mascot").unwrap();
    let switch = visual
        .debug_bounds("settings-toggle-bot-sidebar-show")
        .unwrap();
    let reset = visual.debug_bounds("settings-bot-reset").unwrap();
    assert_eq!(mascot.size, size(px(16.), px(16.)));
    assert_eq!(switch.size, size(px(36.), px(20.)));
    let name = visual.debug_bounds("settings-bot-name").unwrap();
    assert!(mascot.right() < name.left());
    assert!(mascot.left() < px(200.));
    assert!(switch.left() > px(800.));
    assert!(switch.right() <= reset.left());
    assert!(reset.right() <= px(1200.));
    assert!((mascot.top() - switch.top()).abs() < px(4.));
    assert!(
        visual
            .debug_bounds("settings-toggle-bot-sidebar-folders")
            .is_none()
    );
    visual.simulate_click(reset.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-management-dialog").is_some());
    window
        .update(visual, |view, window, cx| {
            view.close_management(window, cx);
            cx.notify();
        })
        .unwrap();
    visual.simulate_resize(size(px(700.), px(800.)));
    visual.run_until_parked();
    let switch = visual
        .debug_bounds("settings-toggle-bot-sidebar-show")
        .unwrap();
    assert_eq!(switch.size, size(px(36.), px(20.)));
    assert!(switch.right() <= px(700.));
}

#[gpui::test]
fn models_menu_hides_and_restores_individual_models(cx: &mut TestAppContext) {
    use vorkspace_core::provider::Kind;
    let window = window(Section::Providers, cx);
    window
        .update(cx, |view, _, cx| {
            view.discovery.binaries = vec![(Kind::Claude, Some("/bin/claude".into()))];
            view.ctx.catalogs.insert(
                "claude".into(),
                vec![("a".into(), "Alpha".into()), ("b".into(), "Beta".into())],
            );
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let at = visual.debug_bounds("models-claude").unwrap().center();
    visual.simulate_click(at, Default::default());
    visual.run_until_parked();
    let row = |visual: &mut VisualTestContext, ix: usize| {
        let selector = ["model-visible-claude-0", "model-visible-claude-1"][ix];
        visual.debug_bounds(selector).unwrap().center()
    };
    let at = row(visual, 1);
    visual.simulate_click(at, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.prefs.hidden_models, ["claude:b"]);
            assert!(view.prefs.model_hidden("claude", "b"));
            assert!(!view.prefs.model_hidden("claude", "a"));
            assert!(!view.prefs.model_hidden("codex", "b"));
        })
        .unwrap();
    // A hidden model stays in the list so it can be restored.
    let at = row(visual, 1);
    visual.simulate_click(at, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.prefs.hidden_models.is_empty())
        })
        .unwrap();
}

#[gpui::test]
fn models_menu_scrolls_to_lower_rows(cx: &mut TestAppContext) {
    use vorkspace_core::provider::Kind;
    let window = window(Section::Providers, cx);
    window
        .update(cx, |view, _, cx| {
            view.discovery.binaries = vec![(Kind::Claude, Some("/bin/claude".into()))];
            view.ctx.catalogs.insert(
                "claude".into(),
                (0..30)
                    .map(|n| (format!("model-{n}"), format!("Model {n}")))
                    .collect(),
            );
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let at = visual.debug_bounds("models-claude").unwrap().center();
    visual.simulate_click(at, Default::default());
    visual.run_until_parked();
    let scroll = visual.debug_bounds("models-scroll-claude").unwrap();
    assert!(scroll.size.height <= px(320.));
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: scroll.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-1200.))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    visual.run_until_parked();
    let last = visual.debug_bounds("model-visible-claude-29").unwrap();
    assert!(last.center().y >= scroll.top() && last.center().y < scroll.bottom());
    visual.simulate_click(last.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.prefs.model_hidden("claude", "model-29"))
        })
        .unwrap();
}

#[gpui::test]
fn providers_without_an_open_conversation_list_their_loaded_models(cx: &mut TestAppContext) {
    use vorkspace_core::provider::Kind;
    let window = window(Section::Providers, cx);
    window
        .update(cx, |view, _, cx| {
            view.discovery.binaries = vec![(Kind::Codex, Some("/bin/codex".into()))];
            view.discovered_catalogs
                .insert("codex".into(), vec![("a".into(), "Alpha".into())]);
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("models-codex").is_some());
    assert!(visual.debug_bounds("settings-select-codex").is_some());
    // Provider defaults are global; there is no project scope selector.
    assert!(
        visual
            .debug_bounds("settings-select-provider-scope")
            .is_none()
    );
}
