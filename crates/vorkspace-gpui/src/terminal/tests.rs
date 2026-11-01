
use super::*;
use alacritty_terminal::index::Line;
use gpui::{AppContext, Modifiers};

fn key(spec: &str) -> Keystroke {
    Keystroke::parse(spec).unwrap()
}
fn bytes(spec: &str) -> Option<Vec<u8>> {
    key_bytes(&key(spec), TermMode::empty())
}

#[test]
fn output_bursts_queue_one_repaint_and_preserve_ordered_effects() {
    let (listener, events) = Listener::channel();
    for _ in 0..10_000 {
        listener.send_event(TermEvent::Wakeup);
        listener.send_event(TermEvent::MouseCursorDirty);
        listener.send_event(TermEvent::CursorBlinkingChange);
    }
    listener.send_event(TermEvent::PtyWrite("reply".into()));
    listener.send_event(TermEvent::ClipboardStore(
        alacritty_terminal::term::ClipboardType::Clipboard,
        "clipboard".into(),
    ));
    listener.send_event(TermEvent::Exit);
    assert_eq!(
        events.len(),
        4,
        "30,000 repaint signals occupy one queue entry"
    );
    assert!(matches!(events.try_recv(), Ok(TermEvent::Wakeup)));
    assert!(matches!(events.try_recv(), Ok(TermEvent::PtyWrite(text)) if text == "reply"));
    assert!(
        matches!(events.try_recv(), Ok(TermEvent::ClipboardStore(_, text)) if text == "clipboard")
    );
    assert!(matches!(events.try_recv(), Ok(TermEvent::Exit)));
    assert!(events.is_empty());

    // Model the consumer's re-arm before reading the latest grid.
    listener.repaint_pending.store(false, Ordering::Release);
    listener.clone().send_event(TermEvent::Wakeup);
    listener.send_event(TermEvent::Wakeup);
    assert_eq!(events.len(), 1, "later output still gets a fresh repaint");
}

#[gpui::test]
fn background_terminal_output_keeps_grid_and_effects_without_repaints(
    cx: &mut gpui::TestAppContext,
) {
    let terminal = cx.new(|cx| Terminal::new("/tmp".into(), cx));
    let notifications = Rc::new(Cell::new(0));
    let _observe = cx.update(|cx| {
        let count = notifications.clone();
        cx.observe(&terminal, move |_, _| count.set(count.get() + 1))
    });
    terminal.update(cx, |terminal, cx| {
        terminal.visible = false;
        write_note(&terminal.term, "background output");
        terminal.on_event(TermEvent::Wakeup, cx);
        terminal.on_event(
            TermEvent::ClipboardStore(
                alacritty_terminal::term::ClipboardType::Clipboard,
                "kept".into(),
            ),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(notifications.get(), 0);
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("kept".into())
    );
    terminal.update(cx, |terminal, cx| {
        let text: String = terminal
            .term
            .lock()
            .grid()
            .display_iter()
            .map(|cell| cell.c)
            .collect();
        assert!(text.contains("background output"));
        terminal.visible = true;
        terminal.on_event(TermEvent::Wakeup, cx);
    });
    cx.run_until_parked();
    assert_eq!(notifications.get(), 1);
}

#[gpui::test]
fn dragging_at_edges_extends_selection_through_history(cx: &mut gpui::TestAppContext) {
    let view = cx.new(|cx| Terminal::new("/tmp".into(), cx));
    view.update(cx, |terminal, cx| {
        let cell = size(px(8.), px(16.));
        terminal.resize(40, 4, cell);
        terminal.geometry.set(Some(Geometry {
            origin: point(px(0.), px(0.)),
            cell,
        }));
        let output = (0..30)
            .map(|n| format!("line-{n:02}"))
            .collect::<Vec<_>>()
            .join("\r\n");
        write_note(&terminal.term, &output);
        terminal.term.lock().selection = Some(Selection::new(
            SelectionType::Simple,
            GridPoint::new(Line(3), Column(6)),
            Side::Right,
        ));
        terminal.selecting = true;
        // Holding a click alone must not scroll.
        assert!(terminal.scroll_selection(cx));
        assert_eq!(terminal.term.lock().grid().display_offset(), 0);
        terminal.selection_pointer = Some(point(px(0.), px(0.)));
        assert!(terminal.scroll_selection(cx));
        assert_eq!(terminal.term.lock().grid().display_offset(), 1);
        let text = terminal.term.lock().selection_to_string().unwrap();
        assert!(text.contains("line-25"), "{text}");
        assert!(text.contains("line-29"), "{text}");
        // A second tick at the same pointer position must keep extending.
        assert!(terminal.scroll_selection(cx));
        assert!(
            terminal
                .term
                .lock()
                .selection_to_string()
                .unwrap()
                .contains("line-24")
        );
        terminal.selection_pointer = Some(point(px(0.), px(32.)));
        terminal.scroll_selection(cx);
        assert_eq!(terminal.term.lock().grid().display_offset(), 2);
        // Dragging past the bottom returns toward the live screen.
        terminal.selection_pointer = Some(point(px(0.), px(80.)));
        terminal.scroll_selection(cx);
        assert_eq!(terminal.term.lock().grid().display_offset(), 0);
        terminal.stop_selection();
        assert!(!terminal.scroll_selection(cx));
    });
}

#[test]
fn keys_encode_like_xterm_with_the_macos_editing_shortcuts() {
    assert_eq!(bytes("enter"), Some(b"\r".to_vec()));
    assert_eq!(bytes("backspace"), Some(b"\x7f".to_vec()));
    assert_eq!(bytes("up"), Some(b"\x1b[A".to_vec()));
    assert_eq!(
        key_bytes(&key("up"), TermMode::APP_CURSOR),
        Some(b"\x1bOA".to_vec())
    );
    assert_eq!(bytes("shift-right"), Some(b"\x1b[1;2C".to_vec()));
    assert_eq!(bytes("ctrl-c"), Some(vec![3]));
    assert_eq!(bytes("ctrl-["), Some(vec![27]));
    assert_eq!(bytes("pagedown"), Some(b"\x1b[6~".to_vec()));
    assert_eq!(bytes("f5"), Some(b"\x1b[15~".to_vec()));
    assert_eq!(bytes("shift-tab"), Some(b"\x1b[Z".to_vec()));
    // terminalKeys.ts
    assert_eq!(bytes("alt-left"), Some(b"\x1bb".to_vec()));
    assert_eq!(bytes("alt-right"), Some(b"\x1bf".to_vec()));
    assert_eq!(bytes("cmd-left"), Some(b"\x01".to_vec()));
    assert_eq!(bytes("cmd-right"), Some(b"\x05".to_vec()));
    assert_eq!(bytes("cmd-backspace"), Some(b"\x15".to_vec()));
    // Option is Meta.
    assert_eq!(bytes("alt-b"), Some(b"\x1bb".to_vec()));
    // App shortcuts stay with the app.
    assert_eq!(bytes("cmd-j"), None);
    assert_eq!(bytes("cmd-shift-d"), None);
    let typed = Keystroke {
        modifiers: Modifiers::default(),
        key: "a".into(),
        key_char: Some("a".into()),
    };
    assert_eq!(key_bytes(&typed, TermMode::empty()), Some(b"a".to_vec()));
}

#[test]
fn foreground_labels_name_the_script_an_interpreter_runs() {
    assert_eq!(
        command_label("node /x/node_modules/.bin/vite --port 3000").as_deref(),
        Some("vite")
    );
    assert_eq!(
        command_label("/usr/bin/python3 -m http.server").as_deref(),
        Some("http.server")
    );
    assert_eq!(
        command_label("cargo watch -x test").as_deref(),
        Some("cargo")
    );
    assert_eq!(command_label("-zsh").as_deref(), Some("zsh"));
    assert!(is_shell_name("zsh"));
    assert_eq!(default_title("/work/vorkspace"), "vorkspace");
    assert_eq!(default_title("/"), "Terminal");
}

#[test]
fn colours_follow_the_dark_palette_and_xterm_cube() {
    assert_eq!(indexed_rgb(1), 0xf87171);
    assert_eq!(indexed_rgb(16), 0x000000);
    assert_eq!(indexed_rgb(196), 0xff0000);
    assert_eq!(indexed_rgb(232), 0x080808);
    assert_eq!(indexed_rgb(255), 0xeeeeee);
    let colors = Colors::default();
    assert_eq!(hex(color_rgb(257, &colors)), OSC_BACKGROUND);
    assert_eq!(hex(color_rgb(258, &colors)), ACCENT);
}

#[test]
fn a_login_shell_runs_and_its_output_reaches_the_grid() {
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    let size = GridSize {
        columns: 60,
        lines: 10,
    };
    let spawned = spawn(&dir, size, 1);
    let sender = spawned.sender.expect("the shell started");
    assert!(spawned.shell_pid > 1);
    let _ = sender.send(Msg::Input(Cow::Borrowed(b"printf 'bot%scode\\n' -\r")));
    let screen = || {
        let term = spawned.term.lock();
        let grid = term.grid();
        (0..grid.screen_lines())
            .map(|line| {
                (0..grid.columns())
                    .map(|c| grid[Line(line as i32)][Column(c)].c)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut seen = false;
    for _ in 0..250 {
        std::thread::sleep(Duration::from_millis(20));
        if screen().contains("bot-code") {
            seen = true;
            break;
        }
    }
    assert!(seen, "{}", screen());
    assert_eq!(spawned.term.lock().columns(), 60);
    // The shell owns the foreground while it waits at a prompt.
    assert_eq!(foreground_label(spawned.master_fd, spawned.shell_pid), None);
    let _ = sender.send(Msg::Input(Cow::Borrowed(b"exit\r")));
    let mut exited = false;
    for _ in 0..250 {
        std::thread::sleep(Duration::from_millis(20));
        if let Ok(TermEvent::ChildExit(_)) = spawned.events.try_recv() {
            exited = true;
            break;
        }
    }
    assert!(exited);
}
