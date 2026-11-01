
use super::*;
use gpui::{TestAppContext, WindowHandle};
#[gpui::test]
fn bot_mentions_replace_only_the_token_without_loading_catalogs_or_submitting(
    cx: &mut TestAppContext,
) {
    let window = cx.update(|cx| {
        input::bind(cx);
        bind(cx);
        crate::popover::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let input = cx.new(input::Input::channel_message);
                Completion::bot_mentions(
                    input,
                    vec![
                        BotMention {
                            id: "alpha".into(),
                            name: "Alpha".into(),
                            mascot: "cat".into(),
                            color: 0xabcdef,
                        },
                        BotMention {
                            id: "beta".into(),
                            name: "Beta Team".into(),
                            mascot: "robot".into(),
                            color: 0xabcdef,
                        },
                    ],
                    cx,
                )
            })
        })
        .unwrap()
    });
    let submitted = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = submitted.clone();
    let _subscription = window
        .update(cx, |view, window, cx| {
            window.focus(&view.input.focus_handle(cx), cx);
            view.input.update(cx, |input, cx| {
                input.set_text("Ask @Be later 🦀".into(), cx);
                input.select_utf16(7..7, cx);
            });
            cx.subscribe(&view.input, move |_, _, _: &input::Submitted, _| {
                count.set(count.get() + 1)
            })
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert_eq!(view.choices.len(), 1);
            assert_eq!(view.choices[0].insertion, "@Beta Team");
            assert!(!view.loading && !view.native_loading);
            assert!(view.files.is_empty() && view.skills.is_empty() && view.notes.is_empty());
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.input.read(cx).content, "Ask @Beta Team later 🦀");
            assert!(view.input.focus_handle(cx).is_focused(window));
            view.input
                .update(cx, |input, cx| input.set_text("@Al".into(), cx));
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.token.is_none());
            assert_eq!(view.input.read(cx).content, "@Al");
            view.set_bots(
                vec![BotMention {
                    id: "beta".into(),
                    name: "Beta Team".into(),
                    mascot: "robot".into(),
                    color: 0xabcdef,
                }],
                cx,
            );
            assert!(view.choices.is_empty());
        })
        .unwrap();
    assert_eq!(submitted.get(), 0);
}
fn window(cx: &mut TestAppContext) -> WindowHandle<Completion> {
    let window = cx.update(|cx| {
        input::bind(cx);
        bind(cx);
        crate::popover::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let input = cx.new(input::Input::new);
                let mut completion = Completion::new(input, "/not-a-real-project".into(), cx);
                completion.loaded = Some(Instant::now());
                completion
            })
        })
        .unwrap()
    });
    window
        .update(cx, |v, w, cx| w.focus(&v.input.focus_handle(cx), cx))
        .unwrap();
    cx.run_until_parked();
    window
}
#[gpui::test]
fn bot_chat_hides_session_mode_suggestions(cx: &mut TestAppContext) {
    let window = window(cx);
    for (query, label) in [
        ("/pla", "Plan mode"),
        ("/operator", "Operator"),
        ("/orchestrator", "Orchestrator"),
    ] {
        window
            .update(cx, |view, _, cx| {
                view.input
                    .update(cx, |input, cx| input.set_text(query.into(), cx))
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| {
                assert!(view.choices.iter().any(|choice| choice.label == label))
            })
            .unwrap();
    }
    window
        .update(cx, |view, _, cx| view.set_bot_chat(cx))
        .unwrap();
    for (query, label) in [
        ("/pla", "Plan mode"),
        ("/operator", "Operator"),
        ("/orchestrator", "Orchestrator"),
    ] {
        window
            .update(cx, |view, _, cx| {
                view.input
                    .update(cx, |input, cx| input.set_text(query.into(), cx))
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| {
                assert!(!view.choices.iter().any(|choice| choice.label == label))
            })
            .unwrap();
    }
}
#[gpui::test]
fn slash_accept_keeps_input_focus_and_does_not_submit(cx: &mut TestAppContext) {
    let window = window(cx);
    let submitted = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = submitted.clone();
    let _submission = window
        .update(cx, |v, _, cx| {
            cx.subscribe(&v.input, move |_, _, _: &input::Submitted, _| {
                count.set(count.get() + 1)
            })
        })
        .unwrap();
    window
        .update(cx, |v, _, cx| {
            v.input.update(cx, |i, cx| i.set_text("/pla".into(), cx))
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    window
        .update(cx, |v, w, cx| {
            assert_eq!(v.input.read(cx).content, "/plan ");
            assert!(v.token.is_none());
            assert!(v.input.focus_handle(cx).is_focused(w));
        })
        .unwrap();
    assert_eq!(submitted.get(), 0);
}
#[gpui::test]
fn compact_completion_inserts_a_local_command_without_mode_selection(cx: &mut TestAppContext) {
    let window = window(cx);
    window
        .update(cx, |view, _, cx| {
            view.input
                .update(cx, |input, cx| input.set_text("/comp".into(), cx));
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.input.read(cx).content, "/compact ")
        })
        .unwrap();
}
#[gpui::test]
fn mcp_selection_replaces_the_command_and_only_present_tags_add_context(cx: &mut TestAppContext) {
    let window = window(cx);
    window
        .update(cx, |view, window, cx| {
            view.input
                .update(cx, |input, cx| input.set_text("/mcp".into(), cx));
            view.loading = true;
            view.mcp = vec![vorkspace_core::mcp::McpConnection {
                provider: "codex".into(),
                name: "docs".into(),
                scope: "project".into(),
                config_path: "/repo/config.toml".into(),
                transport: "stdio".into(),
                enabled: true,
            }];
            view.open_mcp(cx);
            view.pick(0, window, cx);
            assert_eq!(view.input.read(cx).content, "@mcp/docs ");
            assert!(
                view.prepare_prompt("Use @mcp/docs.")
                    .starts_with("MCP context: Use the configured server ")
            );
            assert_eq!(
                view.prepare_prompt("Use @mcp/docs-extra"),
                "Use @mcp/docs-extra"
            );
            assert_eq!(view.prepare_prompt("Ordinary request"), "Ordinary request");
            assert_eq!(view.prepare_prompt("> Use @mcp/docs"), "> Use @mcp/docs");
            assert_eq!(
                view.prepare_prompt("Quote:\n  > @mcp/docs\nOrdinary request"),
                "Quote:\n  > @mcp/docs\nOrdinary request"
            );
        })
        .unwrap();
}
#[gpui::test]
fn ime_composition_does_not_open_a_picker_or_send(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;
    let window = window(cx);
    window
        .update(cx, |v, w, cx| {
            v.input.update(cx, |i, cx| {
                i.replace_and_mark_text_in_range(None, "/pla", None, w, cx)
            })
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
    window
        .update(cx, |v, _, cx| {
            assert!(v.input.read(cx).is_composing());
            assert!(v.token.is_none());
            assert_eq!(v.input.read(cx).content, "/pla");
        })
        .unwrap();
}
#[gpui::test]
fn mention_accept_replaces_only_the_caret_token_and_escape_dismisses(cx: &mut TestAppContext) {
    let window = window(cx);
    window
        .update(cx, |v, _, cx| {
            v.files = vec![ProjectFile {
                name: "my file.rs".into(),
                relative: "src/my file.rs".into(),
                path: "/repo/src/my file.rs".into(),
            }];
            v.input.update(cx, |i, cx| {
                i.set_text("Use @my trailing".into(), cx);
                i.select_range(7..7, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "tab");
    cx.run_until_parked();
    window
        .update(cx, |v, _, cx| {
            assert_eq!(v.input.read(cx).content, "Use @\"src/my file.rs\" trailing");
            v.input.update(cx, |i, cx| i.set_text("@my".into(), cx));
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.run_until_parked();
    window
        .update(cx, |v, _, cx| {
            assert_eq!(v.input.read(cx).content, "@my");
            assert!(v.token.is_none());
        })
        .unwrap();
}
