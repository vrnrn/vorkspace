use super::*;
use gpui::{TestAppContext, WindowHandle, point};
use std::sync::Mutex;
use vorkspace_core::store::Store;
#[gpui::test]
fn combined_picker_paints_effort_and_beside_mode_moves_it_to_its_own_control(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, cx| {
            view.summary.provider = "codex".into();
            view.model = Some("gpt-6.1-sol".into());
            view.settings.effort = Some("xhigh".into());
            view.catalog = vec![Model {
                alias_target: None,
                id: "gpt-6.1-sol".into(),
                name: "GPT-6.1 Sol".into(),
                efforts: vec!["high".into(), "xhigh".into()],
                default_effort: Some("high".into()),
                fast: false,
                thinking: false,
                default: true,
                tiers: Vec::new(),
                controls: Vec::new(),
            }];
            cx.set_global(vorkspace_ui::preferences::Preferences::default());
            cx.notify();
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    let effort = visual
        .debug_bounds("composer-model-effort")
        .expect("combined model picker includes effort");
    let model = window
        .update(visual, |view, _, _| view.model_anchor.get().unwrap())
        .unwrap();
    assert!(effort.left() > model.left() && effort.right() < model.right());
    window
        .update(visual, |_, _, cx| {
            cx.global_mut::<vorkspace_ui::preferences::Preferences>()
                .model_controls = "beside".into();
            cx.notify();
        })
        .unwrap();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("composer-model-effort").is_none());
    window
        .update(visual, |view, _, _| {
            assert!(
                view.bot_ui.setting_anchors[0].get().is_some(),
                "beside mode paints its effort control"
            );
        })
        .unwrap();
}

#[gpui::test]
fn composer_labels_use_the_saved_model_effort_and_the_catalog_default(cx: &mut TestAppContext) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, _| {
            view.summary.provider = "codex".into();
            view.model = Some("gpt-6.1-sol".into());
            view.settings.effort = Some("xhigh".into());
            view.catalog.clear();
            assert_eq!(view.model_label(), "GPT-6.1-Sol");
            assert_eq!(view.trigger_effort_label().as_deref(), Some("Extra High"));
            view.catalog.push(Model {
                alias_target: None,
                id: "gpt-6.1-sol".into(),
                name: "GPT-6.1 Sol".into(),
                efforts: vec!["high".into(), "xhigh".into()],
                default_effort: Some("high".into()),
                fast: false,
                thinking: false,
                default: true,
                tiers: Vec::new(),
                controls: Vec::new(),
            });
            assert_eq!(view.model_label(), "GPT-6.1 Sol");
            assert_eq!(view.trigger_effort_label().as_deref(), Some("Extra High"));
            view.model = None;
            view.settings.effort = None;
            assert_eq!(view.model_label(), "GPT-6.1 Sol");
            assert_eq!(view.trigger_effort_label().as_deref(), Some("High"));
            view.model = Some("gpt-custom-model".into());
            assert_eq!(view.model_label(), "GPT-custom-model");
            assert!(
                view.model_info().is_none(),
                "unknown saved models must not inherit a different model's controls"
            );
            assert!(view.trigger_effort_label().is_none());
        })
        .unwrap();
}

#[gpui::test]
fn tool_paths_follow_restored_and_transferred_working_copies(cx: &mut TestAppContext) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, cx| {
            view.loaded = false;
            view.settings.revision = 0;
            view.restore_worktree(Some("/work/feature".into()), None, cx);
            assert_eq!(
                view.transcript.read(cx).file_link_base.as_deref(),
                Some("/work/feature")
            );
            // History/transfer/workspace refreshes all pass through sync_review.
            view.settings.worktree_cwd = Some("/work/transferred".into());
            view.sync_review(cx);
            assert_eq!(
                view.transcript.read(cx).file_link_base.as_deref(),
                Some("/work/transferred")
            );
            view.settings.worktree_removed = true;
            view.sync_review(cx);
            assert_eq!(
                view.transcript.read(cx).file_link_base.as_deref(),
                Some("/work/vorkspace")
            );
        })
        .unwrap();
}

#[gpui::test]
fn response_target_picker_preserves_model_effort_and_working_copy(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let picked = std::rc::Rc::new(std::cell::RefCell::new(None));
    let result = picked.clone();
    let _subscription = window
        .update(cx, |view, window, cx| {
            let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Transfer, _| {
                *result.borrow_mut() = Some(event.clone())
            });
            view.summary.has_messages = true;
            view.settings.worktree_cwd = Some("/work/isolated-copy".into());
            view.transcript.update(cx, |transcript, cx| {
                transcript.patch(
                    0,
                    vec![
                        vorkspace_core::transcript::Row {
                            role: "user".into(),
                            text: "Review the footer".into(),
                            first: true,
                            sent_at: None,
                        },
                        vorkspace_core::transcript::Row {
                            role: "assistant".into(),
                            text: "The footer is ready.".into(),
                            first: true,
                            sent_at: None,
                        },
                    ],
                    cx,
                )
            });
            let model = Model {
                alias_target: None,
                id: "gpt-review".into(),
                name: "Review model".into(),
                efforts: vec!["low".into(), "high".into()],
                default_effort: Some("low".into()),
                fast: false,
                thinking: false,
                default: true,
                tiers: Vec::new(),
                controls: vec![catalog::ModelControl {
                    id: "context".into(),
                    name: "Context".into(),
                    current: "1m".into(),
                    choices: vec![("1m".into(), "1M".into())],
                }],
            };
            view.make_response_menu(
                1,
                vorkspace_core::response::TransferKind::Handoff,
                AnchorSlot::default(),
                Some(vec![(Kind::Codex, Some(Ok(vec![model])))]),
                window,
                cx,
            );
            cx.notify();
            subscription
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "right right down enter");
    cx.run_until_parked();
    let result = picked.borrow();
    let result = result
        .as_ref()
        .expect("a target is picked through provider, model and effort");
    assert_eq!(result.target.provider, "codex");
    assert_eq!(result.target.model.as_deref(), Some("gpt-review"));
    assert_eq!(result.target.effort.as_deref(), Some("low"));
    assert_eq!(
        result.target.settings.get("context").map(String::as_str),
        Some("1m")
    );
    assert_eq!(
        result.settings.worktree_cwd.as_deref(),
        Some("/work/isolated-copy")
    );
    assert_eq!(
        vorkspace_core::response::request(&result.turn),
        "Review the footer"
    );
}

#[gpui::test]
fn new_tab_shows_its_folders_branch_at_once_and_no_loading_status(cx: &mut TestAppContext) {
    vorkspace_model::git_cache::remember(
        "/work/remembered",
        &vorkspace_core::git::Snapshot {
            current: Some("main".into()),
            ..Default::default()
        },
    );
    let store = Store::open(Path::new(":memory:")).unwrap();
    let summary = store.create("codex", "/work/remembered").unwrap();
    let store = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut c = Conversation::new(summary, store, cx);
                // The first frame of a tab whose history is still loading.
                c.loading = true;
                c
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |c, _, _| {
            assert_eq!(c.branch.as_deref(), Some("main"));
            assert!(c.git.is_some());
        })
        .unwrap();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("provider-status").is_none());
    // Once loaded, a real status (an error here) still shows.
    window
        .update(visual, |c, _, cx| {
            c.loading = false;
            c.status = "Couldn't load".into();
            cx.notify();
        })
        .unwrap();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("provider-status").is_some());
}

#[gpui::test]
fn picked_access_becomes_the_default_for_new_chats(cx: &mut TestAppContext) {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let first = store.create("codex", "/work").unwrap();
    let second = store.create("codex", "/work").unwrap();
    let store = Arc::new(Mutex::new(store));
    cx.update(|cx| cx.set_global(vorkspace_ui::preferences::Preferences::default()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Conversation::new(first, store.clone(), cx))
        })
        .unwrap()
    });
    window
        .update(cx, |c, _, cx| {
            assert_eq!(c.settings.access, AccessMode::Supervised);
            c.menu_choice("access:auto", cx);
        })
        .unwrap();
    cx.update(|cx| {
        assert_eq!(
            cx.global::<vorkspace_ui::preferences::Preferences>()
                .default_access
                .as_deref(),
            Some("auto")
        );
        let next = cx.new(|cx| Conversation::new(second, store, cx));
        assert_eq!(next.read(cx).settings.access, AccessMode::Auto);
    });
}

#[gpui::test]
fn effort_choices_survive_provider_switches_and_new_projects(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(vorkspace_ui::preferences::Preferences::default()));
    let window = conversation(cx);
    window
        .update(cx, |c, _, cx| {
            c.menu_choice("effort:medium", cx);
            c.apply_model_pick(&ModelPicked(Kind::Codex, Some("gpt-6.1-sol".into())), cx);
            c.menu_choice("effort:xhigh", cx);
            c.apply_model_pick(
                &ModelPicked(Kind::Claude, Some("claude-opus-5-5".into())),
                cx,
            );
            assert_eq!(c.settings.effort.as_deref(), Some("medium"));
            c.apply_model_pick(&ModelPicked(Kind::Codex, Some("gpt-6.1-sol".into())), cx);
            assert_eq!(c.settings.effort.as_deref(), Some("xhigh"));
            c.menu_choice("access:auto", cx);
            let encoded =
                serde_json::to_string(cx.global::<vorkspace_ui::preferences::Preferences>())
                    .unwrap();
            let restored: vorkspace_ui::preferences::Preferences =
                serde_json::from_str(&encoded).unwrap();
            assert_eq!(restored.provider_efforts["claude"], "medium");
            assert_eq!(restored.provider_efforts["codex"], "xhigh");
            assert_eq!(restored.default_access.as_deref(), Some("auto"));
            let store = Store::open(Path::new(":memory:")).unwrap();
            let summary = store.create("codex", "/different/project").unwrap();
            let next = cx.new(|cx| Conversation::new(summary, Arc::new(Mutex::new(store)), cx));
            assert_eq!(next.read(cx).settings.effort.as_deref(), Some("xhigh"));
            assert_eq!(next.read(cx).settings.access, AccessMode::Auto);
        })
        .unwrap();
}

fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let summary = store.create("claude", "/work/vorkspace").unwrap();
    let store = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        input::bind(cx);
        crate::popover::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Conversation::new(summary, store, cx))
        })
        .unwrap()
    });
    window
        .update(cx, |c, w, cx| w.focus(&c.input.focus_handle(cx), cx))
        .unwrap();
    cx.run_until_parked();
    window
}
fn open(window: WindowHandle<Conversation>, kind: ComposerMenu, cx: &mut TestAppContext) {
    window
        .update(cx, |c, w, cx| c.toggle_menu(kind, w, cx))
        .unwrap();
    cx.run_until_parked();
}
fn input_focused(window: WindowHandle<Conversation>, cx: &mut TestAppContext) -> bool {
    window
        .update(cx, |c, w, cx| c.input.focus_handle(cx).is_focused(w))
        .unwrap()
}

#[gpui::test]
fn workspace_existing_worktrees_open_as_a_submenu_and_preserve_the_draft(cx: &mut TestAppContext) {
    use vorkspace_core::git::{Snapshot, Worktree};
    cx.update(|cx| {
        input::bind(cx);
        crate::popover::bind(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    let cwd = "/nonexistent/vorkspace-composer-existing-submenu";
    let feature = format!("{cwd}-feature-b");
    vorkspace_model::git_cache::remember(
        cwd,
        &Snapshot {
            current: Some("main".into()),
            worktrees: vec![
                Worktree {
                    path: cwd.into(),
                    branch: Some("main".into()),
                    main: true,
                    ..Default::default()
                },
                Worktree {
                    path: format!("{cwd}-feature-a"),
                    branch: Some("feature-a".into()),
                    ..Default::default()
                },
                Worktree {
                    path: feature.clone(),
                    branch: Some("feature-b".into()),
                    ..Default::default()
                },
                Worktree {
                    path: format!("{cwd}-missing"),
                    branch: Some("missing".into()),
                    missing: true,
                    ..Default::default()
                },
                Worktree {
                    path: format!("{cwd}-detached"),
                    head: "123456789abc".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    );
    let store = Store::open(Path::new(":memory:")).unwrap();
    let summary = store.create("claude", cwd).unwrap();
    let store = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Conversation::new(summary, store, cx))
        })
        .unwrap()
    });
    window
        .update(cx, |c, w, cx| {
            c.input.update(cx, |input, cx| {
                input.set_text("Keep my unsent draft".into(), cx)
            });
            c.settings.new_worktree = true;
            c.toggle_menu(ComposerMenu::Workspace, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_keystrokes("down down right");
    visual.run_until_parked();
    let parent = visual
        .debug_bounds("menu-action-workspace:existing")
        .unwrap();
    let submenu = visual
        .debug_bounds("existing-worktrees-submenu")
        .expect("existing copies open beside the workspace menu");
    assert!(submenu.left() >= parent.right());
    assert!(submenu.size.height > px(0.) && submenu.size.height <= px(320.));
    assert!(
        visual.debug_bounds("git-option-2").is_some(),
        "detached copies remain available"
    );
    assert!(
        visual.debug_bounds("git-option-3").is_none(),
        "main and missing copies are excluded"
    );
    window
        .update(visual, |c, _, _| {
            assert!(c.menu_is(ComposerMenu::Workspace));
            assert!(
                c.git_picker.is_none(),
                "the submenu never replaces its parent"
            );
        })
        .unwrap();

    visual.simulate_keystrokes("left");
    visual.run_until_parked();
    assert!(visual.debug_bounds("existing-worktrees-submenu").is_none());
    assert!(
        visual
            .debug_bounds("menu-action-workspace:existing")
            .is_some()
    );
    visual.simulate_keystrokes("right escape");
    visual.run_until_parked();
    window
        .update(visual, |c, w, cx| {
            assert!(c.menu.is_none());
            assert!(c.input.focus_handle(cx).is_focused(w));
            assert!(
                c.settings.new_worktree,
                "dismissal does not change workspace mode"
            );
            c.toggle_menu(ComposerMenu::Workspace, w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    let existing = visual
        .debug_bounds("menu-action-workspace:existing")
        .unwrap();
    visual.simulate_mouse_move(existing.center(), None, Default::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("existing-worktrees-submenu").is_some(),
        "hover also opens the submenu"
    );
    visual.simulate_input("feature-b");
    visual.run_until_parked();
    assert!(visual.debug_bounds("git-option-0").is_some());
    assert!(visual.debug_bounds("git-option-1").is_none());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    window
        .update(visual, |c, w, cx| {
            assert!(c.menu.is_none());
            assert_eq!(c.execution_cwd(), feature);
            assert_eq!(
                c.transcript.read(cx).file_link_base.as_deref(),
                Some(feature.as_str())
            );
            assert!(!c.settings.new_worktree);
            assert!(!c.busy);
            assert_eq!(c.input.read(cx).content.as_ref(), "Keep my unsent draft");
            assert!(c.input.focus_handle(cx).is_focused(w));
            assert_eq!(
                locked(&c.store)
                    .unwrap()
                    .agent_settings(c.summary.id)
                    .unwrap()
                    .unwrap()
                    .worktree_cwd
                    .as_deref(),
                Some(feature.as_str())
            );
            c.toggle_menu(ComposerMenu::Workspace, w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    let current = visual
        .debug_bounds("menu-action-workspace:current")
        .unwrap();
    visual.simulate_click(current.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |c, _, cx| {
            assert!(c.settings.worktree_cwd.is_none());
            assert_eq!(c.execution_cwd(), cwd);
            assert_eq!(c.input.read(cx).content.as_ref(), "Keep my unsent draft");
        })
        .unwrap();
}

#[gpui::test]
fn mode_menu_edits_the_inline_prefix_and_removal_preserves_the_draft(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;
    let window = conversation(cx);
    window
        .update(cx, |c, _, cx| {
            c.input
                .update(cx, |i, cx| i.set_text("Review this".into(), cx));
            c.toggle_mode("plan", cx);
            assert_eq!(c.input.read(cx).content, "/plan Review this");
            c.toggle_mode("operator", cx);
            assert_eq!(c.input.read(cx).content, "/operator Review this");
            c.toggle_mode("orchestrator", cx);
            assert_eq!(c.input.read(cx).content, "/orchestrator Review this");
            c.toggle_mode("orchestrator", cx);
            assert_eq!(c.input.read(cx).content, "Review this");
            c.toggle_mode("plan", cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |c, w, cx| {
            c.input.update(cx, |i, cx| {
                i.select_range(0..i.content.len(), cx);
                i.replace_text_in_range(None, "/operator New draft", w, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |c, _, cx| {
            assert!(!c.settings.plan);
            assert_eq!(c.input.read(cx).content, "/operator New draft");
            c.enable_mode("operator", cx);
            c.input.update(cx, |i, cx| i.select_range(1..1, cx));
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "backspace");
    cx.run_until_parked();
    window
        .update(cx, |c, _, cx| {
            assert!(!c.settings.operator);
            assert_eq!(c.input.read(cx).content, "operator New draft");
        })
        .unwrap();
}

#[gpui::test]
fn submitted_modes_clear_for_a_started_turn_and_stay_cleared_after_completion(
    cx: &mut TestAppContext,
) {
    for mode in ["operator", "orchestrator", "plan"] {
        let window = conversation(cx);
        window
            .update(cx, |view, _, cx| {
                // An absent directory prevents the supported CLI from starting.
                view.summary.provider = "codex".into();
                view.summary.cwd = "/missing/submitted-mode-project".into();
                view.loaded = true;
                view.input
                    .update(cx, |i, cx| i.set_text("check the composer".into(), cx));
                view.toggle_mode(mode, cx);
                view.send(cx);
                assert!(view.busy);
                assert!(view.input.read(cx).content.is_empty());
                assert!(
                    !view.settings.plan && !view.settings.operator && !view.settings.orchestrator
                );
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                let saved = locked(&view.store)
                    .unwrap()
                    .agent_settings(view.summary.id)
                    .unwrap()
                    .unwrap();
                assert!(!saved.plan && !saved.operator && !saved.orchestrator);
                assert!(view.input.read(cx).content.is_empty());
            })
            .unwrap();
    }
}

#[gpui::test]
fn model_menu_picks_by_keyboard_and_returns_focus(cx: &mut TestAppContext) {
    let window = conversation(cx);
    open(window, ComposerMenu::Model, cx);
    assert!(
        !input_focused(window, cx),
        "the menu takes focus while open"
    );
    // Open the Model row's flyout, which focuses its search field with
    // Default highlighted, then take the first catalog model.
    cx.simulate_keystrokes(window.into(), "right down enter");
    cx.run_until_parked();
    window
        .update(cx, |c, _, _| {
            assert!(c.menu.is_none());
            assert_eq!(c.model.as_deref(), Some("sonnet"));
            // The pill drops the catalog's "Claude " prefix, as React's does.
            assert_eq!(c.model_label(), "Sonnet");
        })
        .unwrap();
    assert!(input_focused(window, cx));

    open(window, ComposerMenu::Model, cx);
    cx.simulate_keystrokes(window.into(), "escape");
    cx.run_until_parked();
    window
        .update(cx, |c, _, _| {
            assert!(c.menu.is_none());
            assert_eq!(c.model.as_deref(), Some("sonnet"));
        })
        .unwrap();
    assert!(input_focused(window, cx));
}

#[gpui::test]
fn recent_models_open_on_right_click_and_pick_by_keyboard(cx: &mut TestAppContext) {
    let window = conversation(cx);
    window
        .update(cx, |c, _, cx| {
            c.model = Some("opus".into());
            let prefs = vorkspace_ui::preferences::Preferences {
                recent_models: [
                    ("codex", Some("gpt-6.1-sol")),
                    ("claude", Some("opus")),
                    ("claude", None),
                    ("codex", Some("gpt-6-luna")),
                ]
                .into_iter()
                .map(|(provider, model)| vorkspace_ui::preferences::ModelChoice {
                    provider: provider.into(),
                    model: model.map(Into::into),
                })
                .collect(),
                hidden_models: vec![
                    vorkspace_ui::preferences::model_key("codex", "gpt-6.1-sol"),
                    vorkspace_ui::preferences::model_key("claude", "opus"),
                ],
                ..Default::default()
            };
            cx.set_global(prefs);
            cx.notify();
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let pill = window
        .update(visual, |c, _, _| c.model_anchor.get().unwrap())
        .unwrap()
        .center();
    visual.simulate_event(gpui::MouseDownEvent {
        position: pill,
        button: MouseButton::Right,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    visual.run_until_parked();
    window
        .update(visual, |c, _, cx| {
            assert!(c.menu_is(ComposerMenu::Recent));
            // Recents with a model, in order; the default is not one.
            let items = c.recent_model_items(cx);
            let labels: Vec<_> = items
                .iter()
                .filter_map(|item| match item {
                    MenuItem::Action(a) => Some((a.label.to_string(), a.checked)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                labels,
                [
                    ("Opus".to_string(), true),
                    ("GPT-6-luna".to_string(), false),
                ]
            );
        })
        .unwrap();
    // The current model is highlighted; Down then Enter takes the next.
    visual.simulate_keystrokes("down enter");
    visual.run_until_parked();
    window
        .update(visual, |c, _, cx| {
            assert!(c.menu.is_none());
            assert_eq!(c.summary.provider, "codex");
            assert_eq!(c.model.as_deref(), Some("gpt-6-luna"));
            let recent = &cx
                .global::<vorkspace_ui::preferences::Preferences>()
                .recent_models[0];
            assert_eq!(recent.model.as_deref(), Some("gpt-6-luna"));
            // App: Switch Model toggles the same menu.
        })
        .unwrap();
    window
        .update(visual, |c, w, cx| c.toggle_recent_models(w, cx))
        .unwrap();
    window
        .update(visual, |c, _, _| assert!(c.menu_is(ComposerMenu::Recent)))
        .unwrap();
    window
        .update(visual, |c, w, cx| c.toggle_recent_models(w, cx))
        .unwrap();
    window
        .update(visual, |c, _, _| assert!(c.menu.is_none()))
        .unwrap();
}

#[gpui::test]
fn pressing_outside_dismisses_and_the_trigger_toggles(cx: &mut TestAppContext) {
    let window = conversation(cx);
    open(window, ComposerMenu::Access, cx);
    cx.update_window(window.into(), |_, w, cx| {
        w.dispatch_event(
            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                position: point(px(1.), px(1.)),
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |c, _, _| assert!(c.menu.is_none()))
        .unwrap();
    open(window, ComposerMenu::Add, cx);
    open(window, ComposerMenu::Add, cx);
    window
        .update(cx, |c, _, _| assert!(c.menu.is_none()))
        .unwrap();
    assert!(input_focused(window, cx));
}
