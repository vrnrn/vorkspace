//! Layout regressions for the shared session/Bot command renderer.
use super::*;
use crate::transcript::Body;
use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext, WindowHandle, point, size};
use vorkspace_core::transcript::Row;

fn command_call(command: &str, state: ToolState) -> ToolCall {
    let mut call = ToolCall::new("wrap", "Bash");
    call.input = serde_json::json!({"command": command}).to_string();
    call.state = state;
    if call.state == ToolState::Failed {
        call.output = "Command failed".into();
    }
    call
}

fn fixture(cx: &mut TestAppContext, bot: bool) -> WindowHandle<TranscriptView> {
    cx.update(|cx| cx.set_reduce_motion(true));
    cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| {
                let mut view = TranscriptView::new();
                if bot {
                    view.bot_mode("cat", 0xe86b5f, "BotCat");
                }
                view
            })
        })
        .unwrap()
    })
}

fn patch(window: WindowHandle<TranscriptView>, cx: &mut TestAppContext, call: &ToolCall) {
    window
        .update(cx, |view, _, cx| {
            view.patch(
                0,
                vec![
                    Row {
                        role: "user".into(),
                        text: "Check the command".into(),
                        first: true,
                        sent_at: None,
                    },
                    Row {
                        role: "tool".into(),
                        text: serde_json::to_string(call).unwrap(),
                        first: true,
                        sent_at: None,
                    },
                ],
                cx,
            );
            view.work_folds.insert(1, true);
            view.phases.insert(1, true);
            view.rebuild(0, cx);
            let rendered = view.rows.iter().find_map(|row| match &row.body {
                Body::Tool { call, .. } => Some(call),
                _ => None,
            });
            assert_eq!(rendered.unwrap().input, call.input);
        })
        .unwrap();
}

// Compare the rendered command bounds to an unclamped text-system layout of
// the complete source at the space actually allocated by the row. Checking
// containment and total line height catches truncation, fixed-height clipping,
// missing multiline tails, and flex children escaping the transcript width.
fn assert_command_layout(
    visual: &mut VisualTestContext,
    selector: &'static str,
    row_selector: &'static str,
    command: &str,
) -> Bounds<Pixels> {
    let bounds = visual.debug_bounds(selector).expect("visible command");
    let row = visual.debug_bounds(row_selector).expect("visible tool row");
    assert!(bounds.size.width > px(0.));
    assert!(bounds.left() >= row.left());
    assert!(bounds.right() <= row.right());
    assert!(bounds.top() >= row.top());
    assert!(bounds.bottom() <= row.bottom());
    visual.update(|window, _| {
        assert!(row.left() >= px(0.));
        assert!(row.right() <= window.viewport_size().width);
        let mut style = window.text_style();
        style.font_family = markdown::FIXED_FONT.into();
        let lines = window
            .text_system()
            .shape_text(
                command.to_owned().into(),
                px(12.),
                &[style.to_run(command.len())],
                Some(bounds.size.width),
                None,
            )
            .unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_ref())
                .collect::<Vec<_>>()
                .join("\n"),
            command,
            "wrapping must retain every byte without inserted break characters"
        );
        let full_height: Pixels = lines.iter().map(|line| line.size(px(20.)).height).sum();
        assert_eq!(bounds.size.height, full_height, "{command}");
    });
    bounds
}

#[gpui::test]
fn raw_commands_wrap_at_narrow_and_wide_widths_in_sessions_and_bots(cx: &mut TestAppContext) {
    let commands = [
        "cargo test --package vorkspace-core --lib tool::presentation -- --nocapture --test-threads=1".to_owned(),
        format!("/private/tmp/{}", "unbrokentoken".repeat(24)),
        "python -c 'print(\"quoted café 🦀 with spaces\")\nprint(\"a second line with Unicode 你好 and \\\"quotes\\\"\")'".into(),
    ];
    for bot in [false, true] {
        let window = fixture(cx, bot);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        for command in &commands {
            for state in [
                ToolState::Running,
                ToolState::Completed,
                ToolState::Failed,
                ToolState::Cancelled,
            ] {
                let call = command_call(command, state);
                assert_eq!(call.presentation().target, *command);
                assert!(call.presentation().steps.is_empty());
                patch(window, visual, &call);
                let mut heights = Vec::new();
                for width in [900., 320., 900.] {
                    visual.simulate_resize(size(px(width), px(1600.)));
                    visual.run_until_parked();
                    let bounds = assert_command_layout(visual, "tool-command-1", "tool-1", command);
                    heights.push(bounds.size.height);
                }
                assert!(
                    heights[1] > heights[0],
                    "narrow commands grow vertically: {command}"
                );
                assert_eq!(
                    heights[2], heights[0],
                    "resizing back restores the row height"
                );
            }
        }
    }
}

#[gpui::test]
fn expanded_chain_commands_wrap_and_keep_their_disclosure_in_sessions_and_bots(
    cx: &mut TestAppContext,
) {
    let first = format!(
        "cargo test --manifest-path /repo/{}/Cargo.toml",
        "long_component".repeat(16)
    );
    let second = "python -c 'print(\"quoted café 🦀 with spaces\")\nprint(\"second line 你好\")'";
    let command = format!("{first} && {second}");
    for bot in [false, true] {
        let window = fixture(cx, bot);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        for state in [ToolState::Running, ToolState::Completed, ToolState::Failed] {
            let call = command_call(&command, state);
            let shown = call.presentation();
            assert_eq!(shown.steps.len(), 2);
            assert_eq!(shown.steps[0].command, first);
            assert_eq!(shown.steps[1].command, second);
            patch(window, visual, &call);
            let mut heights = Vec::new();
            for width in [900., 320., 900.] {
                visual.simulate_resize(size(px(width), px(1600.)));
                visual.run_until_parked();
                let row = visual.debug_bounds("tool-1").unwrap();
                if visual.debug_bounds("tool-steps-1").is_none() {
                    visual.simulate_click(
                        point(row.right() - px(8.), row.center().y),
                        Default::default(),
                    );
                    visual.run_until_parked();
                }
                let first_bounds =
                    assert_command_layout(visual, "tool-step-command-1-0", "tool-step-1-0", &first);
                let second_bounds =
                    assert_command_layout(visual, "tool-step-command-1-1", "tool-step-1-1", second);
                assert!(first_bounds.bottom() <= second_bounds.top());
                heights.push(visual.debug_bounds("tool-steps-1").unwrap().size.height);
            }
            assert!(heights[1] > heights[0]);
            assert_eq!(heights[2], heights[0]);
            let row = visual.debug_bounds("tool-1").unwrap();
            visual.simulate_click(
                point(row.right() - px(8.), row.center().y),
                Default::default(),
            );
            visual.run_until_parked();
            assert!(visual.debug_bounds("tool-steps-1").is_none());
        }
    }
}
