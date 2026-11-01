use super::*;
use gpui::{TestAppContext, WindowBounds, WindowHandle, WindowOptions};

#[gpui::test]
fn existing_field_and_composer_reink_after_theme_changes_without_losing_text(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        let mut preferences = vorkspace_ui::preferences::Preferences::default();
        vorkspace_ui::theme::configure(&preferences, false, true);
        let field = cx.new(|cx| {
            Input::field(
                "Search",
                "Search models",
                vorkspace_ui::theme::content(0.35),
                cx,
            )
        });
        let composer = cx.new(Input::new);
        let original_ink = field
            .read(cx)
            .text_color(true, vorkspace_ui::theme::content(1.));
        field.update(cx, |input, cx| {
            input.set_text("unfinished 🙂 draft".into(), cx);
            input.selection = 0..10;
        });
        for theme in ["light", "dark"] {
            preferences.theme = theme.into();
            vorkspace_ui::theme::configure(&preferences, false, true);
            let input = field.read(cx);
            assert_eq!(
                input.text_color(true, vorkspace_ui::theme::content(1.)),
                vorkspace_ui::theme::content(0.35)
            );
            assert_eq!(
                input.text_color(false, vorkspace_ui::theme::content(1.)),
                vorkspace_ui::theme::content(1.)
            );
            assert_eq!(
                composer
                    .read(cx)
                    .text_color(true, vorkspace_ui::theme::content(1.)),
                vorkspace_ui::theme::content(0.4)
            );
            assert_eq!(input.content, "unfinished 🙂 draft");
            assert_eq!(input.selection, 0..10);
            if theme == "light" {
                assert_ne!(
                    input.text_color(true, vorkspace_ui::theme::content(1.)),
                    original_ink
                );
            }
        }
        vorkspace_ui::theme::configure(&Default::default(), false, true);
    });
}

fn field(cx: &mut TestAppContext, single_line: bool) -> WindowHandle<Input> {
    let window = cx.update(|cx| {
        bind(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(260.), px(100.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(if single_line {
                    Input::search
                } else {
                    Input::new
                })
            },
        )
        .unwrap()
    });
    window
        .update(cx, |input, w, cx| w.focus(&input.focus, cx))
        .unwrap();
    cx.refresh().unwrap();
    window
}
#[gpui::test]
fn bounded_textarea_wheel_scroll_survives_repaint_and_typing_reveals_caret(
    cx: &mut TestAppContext,
) {
    let window = field(cx, false);
    window
        .update(cx, |input, _, cx| {
            input.height = 60.;
            input.min_height = 60.;
            input.max_height = 60.;
            input.set_text("Read each line of these instructions.\n".repeat(30), cx);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let (before, position) = window
        .update(visual, |input, _, _| {
            (input.scroll_y, input.bounds.unwrap().center())
        })
        .unwrap();
    assert!(before > px(80.));
    visual.simulate_event(gpui::ScrollWheelEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(80.))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    visual.run_until_parked();
    let scrolled = window
        .update(visual, |input, _, _| {
            assert!(input.manual_scroll);
            input.scroll_y
        })
        .unwrap();
    assert!(scrolled < before);
    window.update(visual, |_, _, cx| cx.notify()).unwrap();
    visual.run_until_parked();
    assert_eq!(
        window.update(visual, |input, _, _| input.scroll_y).unwrap(),
        scrolled
    );
    visual.simulate_input("Additional instruction");
    visual.run_until_parked();
    window
        .update(visual, |input, _, _| {
            assert!(!input.manual_scroll);
            assert!(input.scroll_y > scrolled);
        })
        .unwrap();
}
#[gpui::test]
fn field_limits_and_disabled_editing_preserve_text(cx: &mut TestAppContext) {
    let window = field(cx, true);
    window
        .update(cx, |input, _, _| input.set_max_chars(48))
        .unwrap();
    replace(window, &format!("{}🙂🙂", "a".repeat(45)), cx);
    window
        .update(cx, |input, _, _| {
            assert_eq!(input.content.encode_utf16().count(), 47);
            input.set_read_only(true);
        })
        .unwrap();
    replace(window, "must not replace while signing in", cx);
    window
        .update(cx, |input, _, _| {
            assert_eq!(input.content.encode_utf16().count(), 47)
        })
        .unwrap();
}
fn replace(window: WindowHandle<Input>, text: &str, cx: &mut TestAppContext) {
    window
        .update(cx, |input, w, cx| {
            input.select_all(&SelectAll, w, cx);
            input.replace_text_in_range(None, text, w, cx);
        })
        .unwrap();
    cx.refresh().unwrap();
}
fn selected(input: &Entity<Input>, cx: &TestAppContext) -> String {
    input.read_with(cx, |i, _| i.content[i.selection.clone()].to_string())
}

#[gpui::test]
fn inline_commands_preserve_text_geometry_wrapping_and_clipboard(cx: &mut TestAppContext) {
    let window = field(cx, false);
    let input = window.root(cx).unwrap();
    window
        .update(cx, |i, _, cx| i.enable_inline_commands(cx))
        .unwrap();
    for (name, icon) in [
        ("plan", "ai-idea"),
        ("operator", "cursor-magic"),
        ("orchestrator", "share"),
    ] {
        let text = format!(
            "/{name} café 🙂\nA long prompt that wraps across several visual lines and keeps the full input width."
        );
        replace(window, &text, cx);
        input.read_with(cx, |i, _| {
            let layout = i.command_layout().unwrap();
            assert_eq!(layout.mode.icon(), icon);
            assert!(i.position(1).x >= px(14.));
            assert_eq!(i.position(name.len() + 2).y, px(0.));
            assert!(i.has_message_text());
            for (at, _) in text.char_indices().skip(1) {
                assert_eq!(i.index(i.position(at)), at, "{name}: offset {at}");
            }
            let next_line = text.find('\n').unwrap() + 1;
            assert_eq!(i.position(next_line).x, px(0.));
            assert!(!i.lines[1].wrap_boundaries().is_empty());
        });
        cx.simulate_keystrokes(window.into(), "cmd-a cmd-c");
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(text.as_str())
        );
    }
    replace(window, "/plan ", cx);
    assert!(!input.read_with(cx, |i, _| i.has_message_text()));
    replace(window, "/ plan", cx);
    assert!(input.read_with(cx, |i, _| i.command_layout().is_none()
        && i.has_message_text()));
}

#[gpui::test]
fn retained_inline_mode_keeps_placeholder_caret_and_ime_in_sync(cx: &mut TestAppContext) {
    let window = field(cx, false);
    let input = window.root(cx).unwrap();
    window
        .update(cx, |i, _, cx| {
            i.enable_inline_commands(cx);
            i.selected_command(Some(Command::Plan), cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    input.read_with(cx, |i, _| {
        assert!(i.content.is_empty());
        assert!(i.position(0).x > px(14.));
        assert_eq!(i.index(i.position(0)), 0);
        assert!(!i.has_message_text());
    });
    window
        .update(cx, |i, w, cx| {
            i.replace_and_mark_text_in_range(None, "日本", Some(2..2), w, cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    input.read_with(cx, |i, _| {
        assert!(i.is_composing());
        assert_eq!(i.index(i.position("日本".len())), "日本".len());
    });
    window
        .update(cx, |i, w, cx| {
            i.replace_text_in_range(None, "日本語", w, cx);
            assert_eq!(i.content.as_ref(), "日本語");
            assert_eq!(i.take(cx).as_deref(), Some("日本語"));
        })
        .unwrap();
}

#[gpui::test]
fn keyboard_selection_dispatches_in_search_and_composer(cx: &mut TestAppContext) {
    let modifier = "cmd";
    let word_modifier = "alt";
    let start = "cmd-left";
    let end = "cmd-right";
    let select_start = "cmd-shift-left";
    let select_end = "cmd-shift-right";
    for single_line in [true, false] {
        let window = field(cx, single_line);
        let input = window.root(cx).unwrap();
        replace(window, "alpha café omega", cx);
        cx.simulate_keystrokes(window.into(), select_start);
        assert_eq!(selected(&input, cx), "alpha café omega");
        assert!(input.read_with(cx, |i, _| i.reversed));
        cx.simulate_keystrokes(
            window.into(),
            &format!("right {word_modifier}-shift-left {modifier}-c"),
        );
        assert_eq!(selected(&input, cx), "omega");
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("omega")
        );
        cx.simulate_keystrokes(
            window.into(),
            &format!("{start} shift-right shift-left {select_end}"),
        );
        assert_eq!(selected(&input, cx), "alpha café omega");
        assert!(!input.read_with(cx, |i, _| i.reversed));
        cx.simulate_keystrokes(window.into(), &format!("{end} {word_modifier}-backspace"));
        assert_eq!(
            input.read_with(cx, |i, _| i.content.to_string()),
            "alpha café "
        );
        replace(window, "café e\u{301}🙂", cx);
        cx.simulate_keystrokes(window.into(), "shift-left shift-left");
        assert_eq!(selected(&input, cx), "e\u{301}🙂");
        cx.simulate_input(window.into(), "X");
        assert_eq!(input.read_with(cx, |i, _| i.content.to_string()), "café X");
    }
}

#[gpui::test]
fn multiline_line_and_document_selection_preserve_the_anchor(cx: &mut TestAppContext) {
    let window = field(cx, false);
    let input = window.root(cx).unwrap();
    replace(window, "first line\nsecond word\nlast line", cx);
    let (start, end, select_start, select_end, doc_start, doc_end) = (
        "cmd-left",
        "cmd-right",
        "cmd-shift-left",
        "cmd-shift-right",
        "cmd-up",
        "cmd-shift-down",
    );
    cx.simulate_keystrokes(window.into(), &format!("{start} up {end} {select_start}"));
    assert_eq!(selected(&input, cx), "second word");
    cx.simulate_keystrokes(window.into(), &format!("left {select_end}"));
    assert_eq!(selected(&input, cx), "second word");
    cx.simulate_keystrokes(window.into(), "left shift-down");
    assert_eq!(selected(&input, cx), "second word\n");
    cx.simulate_keystrokes(window.into(), &format!("{doc_start} {doc_end}"));
    assert_eq!(selected(&input, cx), "first line\nsecond word\nlast line");
    replace(
        window,
        "A long composer line that wraps around the field width several times to check visual line selection.",
        cx,
    );
    assert!(input.read_with(cx, |i, _| !i.lines[0].wrap_boundaries().is_empty()));
    cx.simulate_keystrokes(window.into(), select_start);
    let last_visual_line = selected(&input, cx);
    assert!(
        last_visual_line.ends_with("selection.")
            && last_visual_line.len() < input.read_with(cx, |i, _| i.content.len())
    );
}

#[gpui::test]
fn search_repaints_keep_placeholder_still_and_long_queries_scroll_horizontally(
    cx: &mut TestAppContext,
) {
    let window = field(cx, true);
    let input = window.root(cx).unwrap();
    let bounds = input.read_with(cx, |i, _| i.bounds.unwrap());
    for _ in 0..12 {
        cx.refresh().unwrap();
        input.read_with(cx, |i, _| {
            assert_eq!(
                i.scroll_y,
                px(0.),
                "placeholder must stay on the same baseline"
            );
            assert_eq!(i.bounds.unwrap(), bounds);
        });
    }
    replace(window, &"a long search query 🙂 ".repeat(20), cx);
    input.read_with(cx, |i, _| {
        assert_eq!(i.scroll_y, px(0.));
        assert!(i.scroll_x > px(0.));
        assert_eq!(i.lines.len(), 1);
        assert!(i.lines[0].wrap_boundaries().is_empty());
        let caret = i.position(i.cursor()).x - i.scroll_x;
        assert!(caret >= px(0.) && caret < bounds.size.width);
        let click = bounds.origin + point(px(30.), px(10.));
        let at = i.mouse_index(click);
        assert!(at > 0 && i.content.is_char_boundary(at));
    });
    cx.simulate_keystrokes(window.into(), "cmd-left");
    cx.refresh().unwrap();
    assert_eq!(input.read_with(cx, |i, _| i.scroll_x), px(0.));
    replace(window, "first\nsecond\r\nthird", cx);
    assert_eq!(
        input.read_with(cx, |i, _| i.content.to_string()),
        "first second third"
    );
    replace(window, "", cx);
    for _ in 0..6 {
        cx.refresh().unwrap();
        assert_eq!(
            input.read_with(cx, |i, _| (i.scroll_x, i.scroll_y)),
            (px(0.), px(0.))
        );
    }
}
#[gpui::test]
fn secret_fields_preserve_unicode_value_and_mask_drawing_ime_and_clipboard(
    cx: &mut TestAppContext,
) {
    let window = field(cx, true);
    let secret = "päss🙂e\u{301}";
    replace(window, secret, cx);
    window
        .update(cx, |input, window, cx| {
            input.set_secret(true);
            input.select_all(&SelectAll, window, cx);
            assert_eq!(input.content.as_ref(), secret);
            let (display, _) = input.display_text();
            assert!(!display.is_empty());
            assert!(display.chars().all(|ch| ch == '*'));
            let units = secret.encode_utf16().count();
            let mut actual = None;
            assert_eq!(
                input.text_for_range(0..units, &mut actual, window, cx),
                Some("*".repeat(units))
            );
            assert_eq!(actual, Some(0..units));
            cx.write_to_clipboard(ClipboardItem::new_string("existing clipboard".into()));
            input.copy(&Copy, window, cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("existing clipboard")
            );
            input.set_secret(false);
            assert_eq!(input.display_text().0.as_ref(), secret);
            input.copy(&Copy, window, cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(secret)
            );
        })
        .unwrap();
}
#[gpui::test]
fn ime_replacement_and_grapheme_deletion(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(Input::new))
            .unwrap()
    });
    window
        .update(cx, |input, w, cx| {
            input.replace_text_in_range(None, "a🙂", w, cx);
            input.replace_and_mark_text_in_range(None, "に", Some(1..1), w, cx);
            assert!(input.is_composing());
            assert_eq!(input.content.as_ref(), "a🙂に");
            assert!(
                input.take(cx).is_none(),
                "Enter during composition must not submit"
            );
            input.replace_and_mark_text_in_range(None, "日本", Some(2..2), w, cx);
            assert_eq!(input.content.as_ref(), "a🙂日本");
            assert_eq!(input.selected_text_range(false, w, cx).unwrap().range, 5..5);
            input.replace_text_in_range(None, "日本語", w, cx);
            assert!(!input.is_composing());
            assert_eq!(input.content.as_ref(), "a🙂日本語");
            input.select_all(&SelectAll, w, cx);
            input.replace_text_in_range(None, "e\u{301}🙂", w, cx);
            input.backspace(&Backspace, w, cx);
            assert_eq!(input.content.as_ref(), "e\u{301}");
            input.backspace(&Backspace, w, cx);
            assert!(input.content.is_empty());
            input.replace_text_in_range(None, "line one\nline two", w, cx);
            assert_eq!(input.take(cx).as_deref(), Some("line one\nline two"));
        })
        .unwrap();
}
