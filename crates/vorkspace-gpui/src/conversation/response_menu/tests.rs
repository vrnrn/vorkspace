
use super::*;
use gpui::{TestAppContext, WindowHandle};
use std::sync::Mutex;
use vorkspace_core::store::Store;

#[test]
fn saved_model_ids_compare_across_claude_context_and_catalog_formats() {
    assert!(model_ids_match(
        Kind::Claude,
        "claude:opus-5.5",
        "claude-opus-5-5"
    ));
    assert!(model_ids_match(
        Kind::Claude,
        "claude-fable-5[1m]",
        "claude-fable-5"
    ));
    assert!(!model_ids_match(Kind::Claude, "opus", "claude-opus-5-5"));
    assert!(!model_ids_match(Kind::Codex, "same-5.5", "same-5-5"));
}

fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let summary = store.create("claude", "/work/response-picker").unwrap();
    let window = cx.update(|cx| {
        input::bind(cx);
        crate::popover::bind(cx);
        vorkspace_ui::clock::use_test_clock(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Conversation::new(summary, Arc::new(Mutex::new(store)), cx))
        })
        .unwrap()
    });
    window
        .update(cx, |view, window, cx| {
            view.summary.has_messages = true;
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
                );
            });
            window.focus(&view.input.focus_handle(cx), cx)
        })
        .unwrap();
    cx.run_until_parked();
    window
}

fn model(id: &str, name: &str) -> Model {
    Model {
        id: id.into(),
        name: name.into(),
        alias_target: None,
        efforts: vec!["low".into(), "high".into()],
        default_effort: Some("high".into()),
        fast: false,
        thinking: false,
        default: false,
        tiers: Vec::new(),
        controls: Vec::new(),
    }
}

#[gpui::test]
fn response_picker_loads_only_the_opened_provider_and_keeps_the_flyout(cx: &mut TestAppContext) {
    let window = conversation(cx);
    window
        .update(cx, |view, window, cx| {
            view.response_request = Some((1, TransferKind::SecondOpinion, Instant::now()));
            view.make_response_menu(
                1,
                TransferKind::SecondOpinion,
                AnchorSlot::default(),
                Some(vec![
                    (Kind::Codex, None),
                    (Kind::Claude, None),
                    (Kind::Antigravity, None),
                ]),
                window,
                cx,
            );
            assert!(view.response_menu.as_ref().unwrap().requested.is_empty());
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "right");
    cx.run_until_parked();
    window
        .update(cx, |view, window, cx| {
            let menu = view.response_menu.as_ref().unwrap();
            assert_eq!(menu.requested, HashSet::from([Kind::Codex]));
            let root = menu.menu.entity_id();
            let models = menu.models[&Kind::Codex].clone();
            assert!(models.focus_handle(cx).is_focused(window));
            view.finish_response_models(
                view.response_request,
                Kind::Codex,
                &models,
                Ok(vec![model("review", "Review model")]),
                cx,
            );
            assert_eq!(view.response_menu.as_ref().unwrap().menu.entity_id(), root);
            assert!(models.focus_handle(cx).is_focused(window));
        })
        .unwrap();
    cx.run_until_parked();
    // The now-loaded model can open its effort submenu without reopening
    // the provider picker or waiting for either other catalog.
    cx.simulate_keystrokes(window.into(), "right down enter");
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert!(view.response_menu.is_none()))
        .unwrap();
}

#[gpui::test]
fn response_picker_ignores_old_completions_and_insets_model_rows(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let stale = Some((1, TransferKind::Handoff, Instant::now()));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(1));
    window
        .update(cx, |view, window, cx| {
            let request = Some((1, TransferKind::Handoff, Instant::now()));
            view.response_request = request;
            let anchor = AnchorSlot::default();
            anchor.set(Some(gpui::Bounds::new(
                gpui::point(px(100.), px(300.)),
                gpui::size(px(20.), px(20.)),
            )));
            view.make_response_menu(
                1,
                TransferKind::Handoff,
                anchor,
                Some(vec![(
                    Kind::Codex,
                    Some(Ok(vec![model("review", "Review model")])),
                )]),
                window,
                cx,
            );
            let models = view.response_menu.as_ref().unwrap().models[&Kind::Codex].clone();
            view.finish_response_models(
                stale,
                Kind::Codex,
                &models,
                Err("Stale failure".into()),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "right");
    cx.run_until_parked();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    let row = visual
        .debug_bounds(
            r#"menu-action-{"provider":"codex","model":"review","effort":null,"settings":{}}"#,
        )
        .expect("the stale failure did not replace the model");
    let surface = window
        .update(visual, |view, _, cx| {
            let models = &view.response_menu.as_ref().unwrap().models[&Kind::Codex];
            models.read(cx).surface_region().get().unwrap()
        })
        .unwrap();
    assert_eq!(row.left() - surface.left(), px(5.));
    assert_eq!(surface.right() - row.right(), px(5.));
    assert_eq!(row.top() - surface.top(), px(5.));
    assert_eq!(surface.bottom() - row.bottom(), px(5.));
}

#[gpui::test]
fn response_transfer_menus_hide_models_for_their_provider(cx: &mut TestAppContext) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, cx| {
            cx.set_global(crate::settings::Preferences {
                hidden_models: vec![crate::settings::model_key("claude", "hidden")],
                ..Default::default()
            });
            for kind in [TransferKind::Handoff, TransferKind::SecondOpinion] {
                for provider in [Kind::Claude, Kind::Codex] {
                    let options = view.response_model_items(
                        1,
                        kind,
                        provider,
                        Ok(vec![model("hidden", "Hidden"), model("visible", "Visible")]),
                        cx,
                    );
                    let ids: Vec<_> = options
                        .iter()
                        .filter_map(|option| match option {
                            MenuItem::Action(action) => Some(
                                serde_json::from_str::<Target>(&action.id)
                                    .unwrap()
                                    .model
                                    .unwrap(),
                            ),
                            _ => None,
                        })
                        .collect();
                    assert_eq!(
                        ids,
                        if provider == Kind::Claude {
                            vec!["visible"]
                        } else {
                            vec!["hidden", "visible"]
                        }
                    );
                }
            }
        })
        .unwrap();
}

#[gpui::test]
fn second_opinion_excludes_the_resolved_alias_of_the_current_model(cx: &mut TestAppContext) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, cx| {
            view.transcript.update(cx, |transcript, _| {
                transcript.turns.insert(
                    1,
                    vorkspace_core::agent::TurnInfo {
                        provider: "claude".into(),
                        model: "Opus 5.5".into(),
                        model_id: Some("claude:opus-5-5".into()),
                        ..Default::default()
                    },
                );
            });
            let mut opus = model("opus", "Opus 5.5");
            opus.alias_target = Some("claude-opus-5-5".into());
            let options = view.response_model_items(
                1,
                TransferKind::SecondOpinion,
                Kind::Claude,
                Ok(vec![opus, model("sonnet", "Sonnet 5")]),
                cx,
            );
            assert_eq!(options.len(), 1);
            let MenuItem::Action(action) = &options[0] else {
                panic!("a different model remains");
            };
            assert_eq!(action.label.as_ref(), "Sonnet 5");
            let target: Target = serde_json::from_str(&action.id).unwrap();
            assert_eq!(target.model.as_deref(), Some("sonnet"));
        })
        .unwrap();
}
