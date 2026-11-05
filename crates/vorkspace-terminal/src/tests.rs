use super::*;

fn terminal(cols: u16, rows: u16) -> Terminal {
    let palette = std::array::from_fn(|i| if i == 1 { 0xf87171 } else { i as u32 });
    Terminal::new(
        cols,
        rows,
        &Config {
            scrollback_lines: 2000,
            scrollback_bytes: 8 * 1024 * 1024,
            palette,
            foreground: 0xeeeeee,
            background: 0x171717,
            cursor: 0xaabbcc,
        },
    )
    .unwrap()
}

#[test]
fn streaming_utf8_styles_queries_and_resizing() {
    let mut t = terminal(20, 4);
    let text = "a\u{301} 界 👩‍💻";
    for b in text.as_bytes() {
        t.write(&[*b]);
    }
    t.write(b"\r\n\x1b[1;31mred\x1b[0m\x1b[6n\x1b]11;?\x07");
    let frame = t.frame().unwrap();
    assert!(frame.text().contains("a\u{301}"), "{}", frame.text());
    assert!(frame.rows[0].iter().any(|c| c.text == "界" && c.width == 2));
    assert!(frame.rows[1][0].has(Cell::BOLD));
    assert_eq!(frame.rows[1][0].foreground, 0xf87171);
    let replies = t
        .take_effects()
        .into_iter()
        .filter_map(|e| match e {
            Effect::PtyWrite(b) => Some(b),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    assert!(replies.windows(6).any(|s| s == b"\x1b[2;4R"), "{replies:?}");
    assert!(String::from_utf8_lossy(&replies).contains("rgb:1717/1717/1717"));
    t.resize(10, 8, 9, 18).unwrap();
    let frame = t.frame().unwrap();
    assert_eq!(frame.columns, 10);
    assert!(frame.text().contains("red"));
}

#[test]
fn primary_screen_history_selection_and_alternate_screen_survive_transitions() {
    let mut t = terminal(20, 4);
    t.write(
        (0..30)
            .map(|i| format!("line-{i:02}\r\n"))
            .collect::<String>()
            .as_bytes(),
    );
    t.scroll(3);
    assert_eq!(t.display_offset(), 3);
    t.select(0, 0, false, SelectionKind::Simple, false).unwrap();
    t.select(7, 3, true, SelectionKind::Simple, true).unwrap();
    let selected = t.selection_text().unwrap();
    assert!(selected.contains("line-24"), "{selected}");
    t.scroll(2);
    t.select(0, 0, false, SelectionKind::Simple, true).unwrap();
    let selected = t.selection_text().unwrap();
    assert!(selected.contains("line-22"), "{selected}");
    t.resize(25, 5, 8, 16).unwrap();
    assert!(t.selection_text().is_some());
    t.write(b"\x1b[?1049hALT\x1b[?2004h\x1b[?1h");
    assert!(
        t.modes()
            .contains(Modes::ALT_SCREEN | Modes::APP_CURSOR | Modes::BRACKETED_PASTE)
    );
    assert!(t.frame().unwrap().text().contains("ALT"));
    t.write(b"\x1b[?1049l");
    t.scroll_to_bottom();
    assert!(t.frame().unwrap().text().contains("line-29"));
}

#[test]
fn semantic_selection_copy_and_clipboard_write_use_public_api() {
    let mut t = terminal(30, 4);
    t.write(b"hello world\r\nsecond line");
    t.select(8, 0, false, SelectionKind::Word, false).unwrap();
    assert_eq!(t.selection_text().as_deref(), Some("world"));
    t.select(0, 1, false, SelectionKind::Line, false).unwrap();
    assert_eq!(t.selection_text().as_deref(), Some("second line"));
    t.select_all().unwrap();
    assert_eq!(
        t.selection_text().as_deref(),
        Some("hello world\nsecond line")
    );
    t.write(b"\x1b]52;c;a2VwdA==\x07");
    assert!(
        t.take_effects()
            .contains(&Effect::ClipboardStore("kept".into()))
    );
    t.clear_selection();
    assert!(t.selection_text().is_none());
}

#[test]
fn synchronized_output_captures_the_boundary_frame() {
    let mut t = terminal(20, 4);
    t.write(b"before\x1b[?2026h\r\x1b[2Kafter");
    assert!(t.frame().unwrap().text().contains("before"));
    t.write(b"\x1b[?2026l");
    let frame = t.frame().unwrap().text();
    assert!(frame.contains("after"));
    assert!(!frame.contains("before"));
}

#[test]
fn key_encoding_tracks_live_modes_and_reports_release_only_when_requested() {
    let mut t = terminal(20, 4);
    let input = |name, text, action| Key {
        name,
        text,
        unshifted_codepoint: if name == "a" { 97 } else { 0 },
        shift: false,
        control: false,
        alt: false,
        action,
    };
    assert_eq!(t.key(input("up", "", KeyAction::Press)).unwrap(), b"\x1b[A");
    t.write(b"\x1b[?1h");
    assert_eq!(t.key(input("up", "", KeyAction::Press)).unwrap(), b"\x1bOA");
    assert!(
        t.key(input("a", "a", KeyAction::Release))
            .unwrap()
            .is_empty()
    );
    t.write(b"\x1b[>11u");
    assert_eq!(
        t.key(input("a", "a", KeyAction::Press)).unwrap(),
        b"\x1b[97u"
    );
    assert_eq!(
        t.key(input("a", "a", KeyAction::Release)).unwrap(),
        b"\x1b[97;1:3u"
    );
}

#[test]
fn selection_tracks_output_and_handles_a_discarded_anchor() {
    let mut t = terminal(20, 3);
    t.write(b"one\r\ntwo\r\nthree");
    t.select(0, 0, false, SelectionKind::Simple, false).unwrap();
    t.select(3, 1, false, SelectionKind::Simple, true).unwrap();
    assert_eq!(t.selection_text().as_deref(), Some("one\ntwo"));
    t.write(b"\r\nfour");
    assert_eq!(t.selection_text().as_deref(), Some("one\ntwo"));
    t.write(b"\x1b[3J");
    // Clearing scrollback must not leave a dangling selection endpoint.
    let _ = t.select(0, 0, false, SelectionKind::Simple, true);
    t.select_all().unwrap();
    assert!(t.selection_text().is_some());
}

#[test]
fn theme_defaults_change_live_without_overwriting_osc_or_cell_colors() {
    let mut t = terminal(20, 4);
    t.write(b"default \x1b[31mred\x1b[0m");
    t.set_default_colors(0x202020, 0xffffff, 0x123456).unwrap();
    let light = t.frame().unwrap();
    assert_eq!(light.rows[0][0].foreground, 0x202020);
    assert_eq!(light.rows[0][8].foreground, 0xf87171);
    assert_eq!(light.cursor.unwrap().color, 0x123456);
    t.write(b"\x1b]10;#314159\x07\x1b]12;#246810\x07");
    t.set_default_colors(0xeeeeee, 0x171717, 0xaabbcc).unwrap();
    let dark = t.frame().unwrap();
    assert_eq!(dark.rows[0][0].foreground, 0x314159);
    assert_eq!(dark.cursor.unwrap().color, 0x246810);
    t.write(b"\x1b]110\x07\x1b]112\x07");
    let reset = t.frame().unwrap();
    assert_eq!(reset.rows[0][0].foreground, 0xeeeeee);
    assert_eq!(reset.cursor.unwrap().color, 0xaabbcc);
}
