use super::*;
use crate::transcript::Body;
use gpui::{TestAppContext, point};
use vorkspace_core::transcript::Row;

#[gpui::test]
fn search_pills_and_overflow_count_share_a_single_compact_row(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap()
    });
    window.update(cx, |view, _, cx| {
            let mut call = ToolCall::new("search", "Bash");
            call.input = r#"{"command":"rg -n 'model_settings_menu|render_model_settings|self.models|models.clone' src"}"#.into();
            call.state = ToolState::Completed;
            view.patch(0, vec![
                Row { role: "user".into(), text: "Find model settings".into(), first: true, sent_at: None },
                Row { role: "tool".into(), text: serde_json::to_string(&call).unwrap(), first: true, sent_at: None },
            ], cx);
            view.work_folds.insert(1, true);
            view.phases.insert(1, true);
            view.rebuild(0, cx);
        }).unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    for width in [900., 320.] {
        visual.simulate_resize(gpui::size(px(width), px(600.)));
        visual.run_until_parked();
        let row = visual.debug_bounds("tool-1").expect("search row");
        let first = visual
            .debug_bounds("tool-search-pill-1-0")
            .expect("first pattern");
        let second = visual
            .debug_bounds("tool-search-pill-1-1")
            .expect("second pattern");
        let more = visual
            .debug_bounds("tool-search-more-1")
            .expect("overflow count");
        assert_eq!(first.size.height, px(24.));
        assert_eq!(second.size.height, first.size.height);
        assert_eq!(more.size.height, first.size.height);
        assert_eq!(more.center().y, first.center().y);
        assert!(more.right() <= row.right());
        assert!(visual.debug_bounds("tool-search-pill-1-2").is_none());
        assert!(visual.debug_bounds("tool-input-toggle-1").is_none());
        visual.simulate_click(more.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("tool-1").unwrap().size.height,
            row.size.height
        );
    }
}

#[gpui::test]
fn chained_commands_show_step_chips_and_expand_into_steps(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap()
    });
    window.update(cx, |view, _, cx| {
            let mut call = ToolCall::new("chain", "Bash");
            call.input = serde_json::json!({
                "command": "cd /repo && git status --short && cat package.json && sed -n '1,260p' scripts/desktop/build-native.mjs && sed -n '1,260p' scripts/desktop/dev-native.mjs",
                "description": "Inspect the native build scripts",
            })
            .to_string();
            call.state = ToolState::Completed;
            view.patch(0, vec![
                Row { role: "user".into(), text: "Check the build".into(), first: true, sent_at: None },
                Row { role: "tool".into(), text: serde_json::to_string(&call).unwrap(), first: true, sent_at: None },
            ], cx);
            view.work_folds.insert(1, true);
            view.phases.insert(1, true);
            view.rebuild(0, cx);
        }).unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    for width in [900., 320.] {
        visual.simulate_resize(gpui::size(px(width), px(600.)));
        visual.run_until_parked();
        let row = visual.debug_bounds("tool-1").expect("chain row");
        // `cd` is plumbing: git status, package.json, build-native.mjs, then +1.
        let first = visual
            .debug_bounds("tool-step-chip-1-0")
            .expect("first chip");
        let more = visual.debug_bounds("tool-step-more-1").expect("overflow");
        assert!(visual.debug_bounds("tool-step-chip-1-2").is_some());
        assert!(visual.debug_bounds("tool-step-chip-1-3").is_none());
        assert_eq!(more.center().y, first.center().y);
        assert!(more.right() <= row.right());
        assert!(row.size.height <= px(32.));
        assert!(visual.debug_bounds("tool-steps-1").is_none());
    }
    let row = visual.debug_bounds("tool-1").unwrap();
    visual.simulate_click(
        point(row.right() - px(8.), row.center().y),
        Default::default(),
    );
    visual.run_until_parked();
    let steps = visual.debug_bounds("tool-steps-1").expect("expanded steps");
    assert!(steps.size.height >= px(100.));
}

#[gpui::test]
fn grouped_tool_rows_stay_compact_when_clicked(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            let row = |role: &str, text: String| Row {
                role: role.into(),
                text,
                first: true,
                sent_at: None,
            };
            let mut call = ToolCall::new("read", "commandExecution");
            call.input = r#"{"command":"/bin/zsh -lc 'cat src/lib.rs'","cwd":"/repo"}"#.into();
            call.output = "pub fn main() {}".into();
            call.state = ToolState::Completed;
            let mut failed = ToolCall::new("run", "Bash");
            failed.input = r#"{"command":"npm run build"}"#.into();
            failed.output = "Build failed".into();
            failed.state = ToolState::Failed;
            view.patch(
                0,
                vec![
                    row("user", "Inspect".into()),
                    row("tool", serde_json::to_string(&call).unwrap()),
                    row("tool", serde_json::to_string(&failed).unwrap()),
                    row("assistant", "Done".into()),
                ],
                cx,
            );
            view.work_folds.insert(1, true);
            view.phases.insert(1, true);
            view.rebuild(0, cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let caption = visual
        .debug_bounds("tool-1")
        .expect("the grouped tool is visible");
    // Clicking the caption does not open raw command/output details.
    visual.simulate_click(
        point(caption.right() - px(4.), caption.center().y),
        Default::default(),
    );
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(!view.expanded.contains(&1));
            let calls: Vec<_> = view
                .rows
                .iter()
                .filter_map(|row| match &row.body {
                    Body::Tool { call, .. } => Some(call),
                    _ => None,
                })
                .collect();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].presentation().action, "Read");
            assert_eq!(calls[1].state, ToolState::Failed);
        })
        .unwrap();
    assert!(visual.debug_bounds("tool-target-1").is_some());
    assert!(visual.debug_bounds("tool-input-toggle-1").is_none());
}

#[gpui::test]
fn reasoning_and_tools_share_the_phase_rail_and_command_count(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| TranscriptView::new())
        })
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            let row = |role: &str, text: &str| Row {
                role: role.into(),
                text: text.into(),
                first: true,
                sent_at: None,
            };
            let tool = |id: &str, name: &str, input: &str| {
                let mut call = ToolCall::new(id, name);
                call.input = input.into();
                call.state = ToolState::Completed;
                row("tool", &serde_json::to_string(&call).unwrap())
            };
            view.patch(
                0,
                vec![
                    row("user", "Check"),
                    row("assistant", "I'll inspect the connector."),
                    tool("run1", "Bash", r#"{"command":"pwd"}"#),
                    tool("read", "Read", r#"{"file_path":"src/transcript.rs"}"#),
                    row("reasoning", " \n"),
                    tool("find", "Grep", r#"{"pattern":"fn tool"}"#),
                    row(
                        "reasoning",
                        "**The connector is shared.**\n\nCheck its callers.",
                    ),
                    tool("run2", "Bash", r#"{"command":"cargo test"}"#),
                    row("assistant", "Done"),
                ],
                cx,
            );
            view.work_folds.insert(1, true);
            view.phases.insert(2, true);
            view.expanded.insert(6);
            view.rebuild(0, cx);
            let phases: Vec<_> = view
                .rows
                .iter()
                .filter_map(|row| match &row.body {
                    Body::Phase { title, group, .. } => Some((title.as_str(), *group)),
                    _ => None,
                })
                .collect();
            assert_eq!(phases, [("Ran 2 commands · Explored the project", 2)]);
            assert!(!view.rows.iter().any(|row| row.key.0 == 4));
            let thought: Vec<_> = view.rows.iter().filter(|row| row.key.0 == 6).collect();
            assert!(
                thought
                    .iter()
                    .any(|row| matches!(row.body, Body::Reasoning))
            );
            assert!(
                thought
                    .iter()
                    .any(|row| matches!(row.body, Body::Markdown(_)))
            );
            assert!(thought.iter().all(|row| row.phase_group() == Some(2)));
            assert_eq!(
                view.rows
                    .iter()
                    .filter(|row| matches!(row.body, Body::Tool { .. }))
                    .count(),
                4
            );
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("thinking-4").is_none());
    assert!(visual.debug_bounds("thinking-6").is_some());
    assert!(visual.debug_bounds("tool-7").is_some());
}

#[gpui::test]
fn first_assistant_prose_replaces_all_streamed_initial_thoughts(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let view = cx.new(|_| TranscriptView::new());
    view.update(cx, |view, cx| {
        let row = |role: &str, text: &str| Row {
            role: role.into(),
            text: text.into(),
            first: true,
            sent_at: None,
        };
        view.streaming(true, cx);
        view.patch(
            0,
            vec![
                row("user", "Check"),
                row("reasoning", "Plan one"),
                row("reasoning", "Plan two"),
            ],
            cx,
        );
        view.phases.insert(1, true);
        view.rebuild(0, cx);
        assert_eq!(
            view.rows
                .iter()
                .filter(|r| matches!(r.body, Body::Reasoning))
                .count(),
            2
        );
        view.patch(3, vec![row("assistant", "I'll inspect the connector.")], cx);
    });
    TranscriptView::settle_animation_frames(&view, cx);
    view.update(cx, |view, _| {
        assert!(
            !view
                .rows
                .iter()
                .any(|r| matches!(r.body, Body::Reasoning | Body::Phase { .. }))
        );
    });
}
