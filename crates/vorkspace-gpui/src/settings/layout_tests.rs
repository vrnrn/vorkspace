use super::*;
use gpui::{
    Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions, point,
    size,
};

fn catalog_model(id: &str, name: &str) -> vorkspace_core::catalog::Model {
    vorkspace_core::catalog::Model {
        id: id.into(),
        name: name.into(),
        alias_target: None,
        efforts: vec!["low".into(), "high".into()],
        default_effort: Some("low".into()),
        fast: false,
        thinking: false,
        default: false,
        tiers: Vec::new(),
        controls: Vec::new(),
    }
}

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
            view.discovered_catalogs.insert(
                (
                    Kind::Codex,
                    Default::default(),
                    vorkspace_core::catalog::generation(Kind::Codex, &Default::default()),
                ),
                vec![catalog_model("a", "Alpha")],
            );
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

#[gpui::test]
fn discovered_catalog_follows_the_selected_account_and_keeps_profiles_separate(
    cx: &mut TestAppContext,
) {
    use vorkspace_core::provider::Kind;
    let directory = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(&directory.path().join("native.sqlite")).unwrap(),
    ));
    cx.update(|cx| crate::accounts::install(store, cx));
    let window = window(Section::Providers, cx);
    let account = cx.update(|cx| {
        crate::accounts::get(cx).unwrap().update(cx, |center, _| {
            center.book.add(Kind::Codex, "Work").unwrap()
        })
    });
    window
        .update(cx, |view, _, cx| {
            // Keep the test independent of external provider binaries.
            view.discovery.binaries.clear();
            let center = crate::accounts::get(cx).unwrap();
            let default = center
                .read(cx)
                .selected(Kind::Codex, &view.selected_project);
            let work = center.read(cx).profile(&account.id);
            view.discovered_catalogs.insert(
                (
                    Kind::Codex,
                    default.clone(),
                    vorkspace_core::catalog::generation(Kind::Codex, &default),
                ),
                vec![catalog_model("default-model", "Default model")],
            );
            view.discovered_catalogs.insert(
                (
                    Kind::Codex,
                    work.clone(),
                    vorkspace_core::catalog::generation(Kind::Codex, &work),
                ),
                vec![catalog_model("work-model", "Work model")],
            );
            assert_eq!(
                view.discovered_models(Kind::Codex, cx).unwrap()[0].id,
                "default-model"
            );
            vorkspace_core::catalog::invalidate(Kind::Codex, &default);
            assert!(view.discovered_models(Kind::Codex, cx).is_none());
            view.discovered_catalogs.insert(
                (
                    Kind::Codex,
                    default.clone(),
                    vorkspace_core::catalog::generation(Kind::Codex, &default),
                ),
                vec![catalog_model("new-default-model", "New default model")],
            );
            center.update(cx, |center, cx| {
                center
                    .book
                    .select(Kind::Codex, &view.selected_project, &account.id)
                    .unwrap();
                cx.notify();
            });
            assert_eq!(
                view.discovered_models(Kind::Codex, cx).unwrap()[0].id,
                "work-model"
            );
            center.update(cx, |center, cx| {
                center
                    .book
                    .select(Kind::Codex, &view.selected_project, "default")
                    .unwrap();
                cx.notify();
            });
            assert_eq!(
                view.discovered_models(Kind::Codex, cx).unwrap()[0].id,
                "new-default-model"
            );
        })
        .unwrap();
}

#[gpui::test]
fn cost_monitoring_link_setting_appears_only_for_aws_bedrock(cx: &mut TestAppContext) {
    use crate::accounts::Key;
    use vorkspace_core::{provider::Kind, usage::Snapshot};
    let directory = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(&directory.path().join("native.sqlite")).unwrap(),
    ));
    cx.update(|cx| crate::accounts::install(store, cx));
    let window = window(Section::Providers, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("bedrock-cost-link").is_none());
    visual.update(|_, cx| {
        crate::accounts::get(cx).unwrap().update(cx, |center, cx| {
            let key = Key::new(Kind::Claude, center.profile("default"), None);
            center.usage.entry(key).or_default().complete(Ok(Snapshot {
                external_billing: Some("AWS Bedrock".into()),
                updated_at: vorkspace_core::usage::now_ms(),
                ..Default::default()
            }));
            cx.notify();
        });
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("bedrock-cost-link").is_some());
}

#[gpui::test]
fn a_project_setup_command_saves_edits_and_clears(cx: &mut TestAppContext) {
    let window = window(Section::Worktrees, cx);
    let submit = |text: &'static str, cx: &mut TestAppContext| {
        window
            .update(cx, |view, window, cx| {
                view.selected_project = "/project".into();
                view.open_management(management::Target::WorktreeSetup, window, cx);
                let name = view.management.as_ref().unwrap().name.clone();
                let existing = name.read(cx).content.clone();
                name.update(cx, |input, cx| input.set_text(text.to_owned(), cx));
                view.submit_management(cx);
                assert!(view.management.is_none());
                existing
            })
            .unwrap()
    };
    assert_eq!(submit("  npm ci  ", cx), "");
    let saved = window
        .read_with(cx, |view, _| view.prefs.worktree_setup.clone())
        .unwrap();
    assert_eq!(saved["/project"], "npm ci");
    assert_eq!(submit("", cx), "npm ci");
    let saved = window
        .read_with(cx, |view, _| view.prefs.worktree_setup.clone())
        .unwrap();
    assert!(saved.is_empty());
}

#[gpui::test]
fn summary_picker_saves_effort_and_clears_it_when_changing_models(cx: &mut TestAppContext) {
    use vorkspace_core::provider::Kind;
    use vorkspace_meetings::{Meetings, Settings};
    let directory = tempfile::tempdir().unwrap();
    let meetings = Meetings::open(directory.path()).unwrap();
    meetings
        .save_settings(&Settings {
            provider: "codex".into(),
            model: "alpha".into(),
            ..Default::default()
        })
        .unwrap();
    let window = window(Section::Notes, cx);
    window
        .update(cx, |view, _, cx| {
            view.ctx.data_dir = Some(directory.path().into());
            let mut beta = catalog_model("beta", "Beta");
            beta.efforts.clear();
            beta.default_effort = None;
            view.discovered_catalogs.insert(
                (
                    Kind::Codex,
                    Default::default(),
                    vorkspace_core::catalog::generation(Kind::Codex, &Default::default()),
                ),
                vec![catalog_model("alpha", "Alpha"), beta],
            );
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let click = |visual: &mut VisualTestContext, selector: &'static str| {
        let target = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(target, Default::default());
        visual.run_until_parked();
    };
    click(visual, "settings-select-meeting-effort");
    click(visual, "settings-option-meeting-effort-2");
    click(visual, "save-meeting-settings");
    assert_eq!(meetings.settings().unwrap().effort.as_deref(), Some("high"));
    click(visual, "settings-select-meeting-models");
    click(visual, "settings-option-meeting-models-2");
    assert!(
        visual
            .debug_bounds("settings-select-meeting-effort")
            .is_none()
    );
    click(visual, "save-meeting-settings");
    let saved = Meetings::open(directory.path())
        .unwrap()
        .settings()
        .unwrap();
    assert_eq!(saved.model, "beta");
    assert_eq!(saved.effort, None);
    click(visual, "settings-select-meeting-models");
    click(visual, "settings-option-meeting-models-3");
    assert!(visual.debug_bounds("summary-custom-model").is_some());
}

/// Native screenshots use disposable data and never start recording or inference.
#[test]
#[ignore = "Requires Metal and VORKSPACE_SETTINGS_CAPTURE_DIR"]
fn native_settings_captures() {
    use std::sync::Arc;
    use vorkspace_core::provider::Kind;
    struct Fixture {
        view: Entity<SettingsView>,
        nav: bool,
        liquid: crate::liquid::Liquid,
        _observe: Subscription,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .bg(vorkspace_ui::theme::shell_fill())
                .when(self.nav, |d| {
                    d.child(div().w(px(180.)).h_full().flex_shrink_0().child(nav(
                        self.view.read(cx).section,
                        &self.view.read(cx).visible_sections(),
                        |_, _, _| {},
                        |_, _| {},
                        &self.liquid,
                    )))
                })
                .child(div().min_w_0().flex_1().h_full().child(self.view.clone()))
        }
    }
    let output =
        std::path::PathBuf::from(std::env::var_os("VORKSPACE_SETTINGS_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let directory = tempfile::tempdir().unwrap();
    vorkspace_meetings::Meetings::open(directory.path())
        .unwrap()
        .save_settings(&vorkspace_meetings::Settings {
            provider: "codex".into(),
            model: "gpt-6.1-terra".into(),
            effort: Some("high".into()),
            ..Default::default()
        })
        .unwrap();
    for (appearance, light) in [("dark", false), ("light", true)] {
        for (viewport, width, show_nav) in [("wide", 1200., true), ("narrow", 480., false)] {
            for section in Section::ALL {
                if light
                    && !matches!(
                        section,
                        Section::Notes | Section::Providers | Section::Github | Section::Vox
                    )
                {
                    continue;
                }
                let prefs = Preferences {
                    theme: appearance.into(),
                    notifications: false,
                    ..Default::default()
                };
                vorkspace_ui::theme::configure(&prefs, light, false);
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
                    .open_window(size(px(width), px(1000.)), |_, cx| {
                        let view = cx.new(|cx| {
                            let mut view = SettingsView::new(
                                section,
                                prefs,
                                SettingsContext {
                                    data_dir: Some(directory.path().into()),
                                    inbox_connections: [
                                        ("github".into(), (true, "Connected".into())),
                                        ("gitlab".into(), (false, "Not connected".into())),
                                        ("jira".into(), (false, "Not connected".into())),
                                    ]
                                    .into(),
                                    ..Default::default()
                                },
                                cx,
                            );
                            view.discovery_task = None;
                            view.discovery_loading = false;
                            view.discovery.binaries =
                                vec![(Kind::Codex, Some("/bin/codex".into()))];
                            view.discovered_catalogs.insert(
                                (
                                    Kind::Codex,
                                    Default::default(),
                                    vorkspace_core::catalog::generation(
                                        Kind::Codex,
                                        &Default::default(),
                                    ),
                                ),
                                vec![catalog_model("gpt-6.1-terra", "GPT-6.1 Terra")],
                            );
                            view
                        });
                        cx.new(|cx| Fixture {
                            _observe: cx.observe(&view, |_, _, cx| cx.notify()),
                            view,
                            nav: show_nav,
                            liquid: Default::default(),
                        })
                    })
                    .unwrap();
                cx.run_until_parked();
                vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(output.join(format!("{}-{appearance}-{viewport}.png", section.id())))
                    .unwrap();
                if section == Section::Connectors {
                    window
                        .update(&mut cx, |fixture, _, cx| {
                            fixture.view.update(cx, |view, cx| {
                                let mut context = view.ctx.clone();
                                context.inbox_connectors.set("gitlab", false).unwrap();
                                context.inbox_connectors.set("jira", false).unwrap();
                                view.set_context(context, cx);
                                view.update(cx, |prefs| prefs.todos = false);
                            });
                        })
                        .unwrap();
                    cx.run_until_parked();
                    vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(output.join(format!("plugins-disabled-{appearance}-{viewport}.png")))
                        .unwrap();
                }
            }
        }
    }
}
