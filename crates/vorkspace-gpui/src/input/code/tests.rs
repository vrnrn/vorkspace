
use super::*;
use gpui::{TestAppContext, WindowBounds, WindowOptions};
#[gpui::test]
fn wrapped_editor_keeps_unicode_selection_visual_navigation_and_resize_in_sync(
    cx: &mut TestAppContext,
) {
    let window = cx.update(|cx| {
        bind(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(400.), px(240.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(Input::editor),
        )
        .unwrap()
    });
    let text = format!("{}🙂漢字\nsecond line\n", "word ".repeat(30));
    window
        .update(cx, |i, w, cx| {
            w.focus(&i.focus, cx);
            i.set_text(text.clone(), cx);
            i.move_to(0, cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    let height = window
        .update(cx, |i, _, _| {
            let code = i.code.as_ref().unwrap();
            assert!(code.tops[1] - code.tops[0] > i.line_height * 2.);
            for (at, _) in text
                .char_indices()
                .take_while(|(at, _)| *at < text.find('\n').unwrap())
            {
                assert_eq!(i.code_index(i.code_position(at)), at, "UTF-8 offset {at}");
            }
            code.total()
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "down");
    window
        .update(cx, |i, _, _| {
            assert_eq!(
                i.cursor_location().0,
                1,
                "Down moves within a wrapped logical line"
            );
            assert!(i.cursor() > 0);
            assert_eq!(
                i.code_position(i.cursor()).y,
                px(TOP_PADDING + i.line_height)
            );
        })
        .unwrap();
    cx.simulate_window_resize(window.into(), size(px(260.), px(240.)));
    cx.refresh().unwrap();
    window
        .update(cx, |i, _, cx| {
            assert!(i.code.as_ref().unwrap().total() > height);
            i.set_word_wrap(false, cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |i, _, _| {
            assert_eq!(i.content.as_ref(), text);
            assert_eq!(
                i.code.as_ref().unwrap().tops[1] - TOP_PADDING,
                i.line_height
            );
        })
        .unwrap();
}

#[gpui::test]
fn overview_markers_jump_and_the_thumb_drags_without_editing_or_selecting(cx: &mut TestAppContext) {
    use vorkspace_editor::editor_analysis::{Change, Diagnostic};
    let window = cx.update(|cx| {
        bind(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(400.), px(240.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(Input::editor),
        )
        .unwrap()
    });
    let text: String = (0..200).map(|n| format!("line {n}\n")).collect();
    let diagnostic = text.find("line 150").unwrap();
    let change = text.find("line 80").unwrap();
    window
        .update(cx, |i, w, cx| {
            w.focus(&i.focus, cx);
            i.set_text(text.clone(), cx);
            i.set_editor_analysis(
                Analysis {
                    diagnostics: vec![Diagnostic {
                        from: diagnostic,
                        to: diagnostic + 4,
                        severity: "error".into(),
                        message: "Unexpected token".into(),
                    }],
                    changes: vec![Change {
                        kind: "mod".into(),
                        from: change,
                        to: change + 7,
                        deleted: 1,
                        original: "old line\n".into(),
                        old_line: 80,
                        tokens: Vec::new(),
                    }],
                    folds: Vec::new(),
                    tokens: Vec::new(),
                },
                cx,
            );
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let (rail, error_tick, change_tick) = window
        .update(visual, |i, _, _| {
            let code = i.code.as_ref().unwrap();
            (
                code.rail.unwrap(),
                code.markers[0].bounds.center(),
                code.markers[1].bounds.center(),
            )
        })
        .unwrap();
    assert_eq!(rail.size.width, px(18.));
    visual.simulate_click(error_tick, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |i, _, _| {
            assert_eq!(i.cursor(), diagnostic);
            assert!(i.scroll_y > px(2000.));
            assert!(!i.selecting);
        })
        .unwrap();
    visual.simulate_click(change_tick, Default::default());
    visual.run_until_parked();
    let thumb = window
        .update(visual, |i, _, _| {
            assert_eq!(i.cursor(), diagnostic, "Git navigation preserves the caret");
            assert!(i.scroll_y < px(2000.));
            let code = i.code.as_ref().unwrap();
            let m = Metrics::new(
                i.scroll_y.as_f32(),
                code.total(),
                rail.size.height.as_f32(),
                rail.size.height.as_f32(),
            );
            // Grab the thumb away from the change tick, which deliberately
            // takes priority over dragging in the frontend.
            rail.origin + point(px(8.), px(m.thumb_top + 4.))
        })
        .unwrap();
    visual.simulate_mouse_down(thumb, MouseButton::Left, Default::default());
    let end = point(thumb.x, rail.bottom() - px(1.));
    visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |i, _, _| {
            assert_eq!(i.content.as_ref(), text);
            assert_eq!(i.cursor(), diagnostic);
            assert!(i.scroll_y > px(3500.), "drag ended at {:?}", i.scroll_y);
            assert!(i.code.as_ref().unwrap().drag_offset.is_none());
            assert!(!i.selecting);
        })
        .unwrap();
}
#[gpui::test]
fn editor_keyboard_undo_ime_and_viewport_are_native_and_bounded(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        bind(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(400.), px(240.)),
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(Input::editor),
        )
        .unwrap()
    });
    window
        .update(cx, |input, w, cx| {
            w.focus(&input.focus, cx);
            input.set_text((0..10_000).map(|n| format!("line {n}: 🙂\n")).collect(), cx);
            input.move_to(0, cx);
            input.reset_editor_history();
        })
        .unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |input, _, _| {
            assert!(input.code.as_ref().unwrap().visible.len() < 20);
            assert_eq!(input.cursor_location(), (1, 1));
        })
        .unwrap();
    let m = "cmd";
    cx.simulate_keystrokes(window.into(), "tab enter");
    window
        .update(cx, |i, _, _| assert!(i.content.starts_with("  \n  line 0")))
        .unwrap();
    cx.simulate_keystrokes(window.into(), &format!("{m}-z {m}-z"));
    window
        .update(cx, |i, _, _| assert!(i.content.starts_with("line 0")))
        .unwrap();
    cx.simulate_keystrokes(window.into(), &format!("{m}-shift-z"));
    window
        .update(cx, |i, w, cx| {
            assert!(i.content.starts_with("  line 0"));
            i.replace_and_mark_text_in_range(None, "漢", None, w, cx);
            i.replace_text_in_range(None, "漢字", w, cx);
            assert!(i.content.starts_with("  漢字line"));
            i.undo(&Undo, w, cx);
            assert!(i.content.starts_with("  line"));
            i.move_to(i.content.len(), cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |i, _, cx| {
            assert!(i.scroll_y > px(100_000.));
            let code = i.code.as_mut().unwrap();
            assert!(code.visible.len() < 20);
            i.scroll_y = px(0.);
            code.reveal = false;
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |i, w, cx| {
            assert_eq!(
                i.scroll_y,
                px(0.),
                "scrolling away must not snap back to the caret"
            );
            i.move_to(0, cx);
            let before = i.content.clone();
            i.replace_text_in_range(None, &"x".repeat(document_limit()), w, cx);
            assert_eq!(
                i.content, before,
                "oversized edits must be rejected in full"
            );
        })
        .unwrap();
}
fn document_limit() -> usize {
    vorkspace_core::files::document::MAX_LINE_BYTES + 1
}
