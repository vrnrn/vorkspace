use super::*;
use gpui::{AppContext, Modifiers};

fn key(spec: &str) -> Keystroke {
    Keystroke::parse(spec).unwrap()
}
fn bytes(spec: &str) -> Option<Vec<u8>> {
    key_bytes(
        &key(spec),
        &mut engine(GridSize {
            columns: 80,
            lines: 24,
        })
        .lock()
        .unwrap(),
    )
}

#[test]
fn output_bursts_queue_one_repaint_and_preserve_ordered_effects() {
    let (listener, events) = Listener::channel();
    for _ in 0..10_000 {
        listener.send_event(TermEvent::Wakeup);
        listener.send_event(TermEvent::Wakeup);
        listener.send_event(TermEvent::Wakeup);
    }
    listener.send_event(TermEvent::PtyWrite("reply".into()));
    listener.send_event(TermEvent::ClipboardStore("clipboard".into()));
    listener.send_event(TermEvent::Exit);
    assert_eq!(
        events.len(),
        4,
        "30,000 repaint signals occupy one queue entry"
    );
    assert!(matches!(events.try_recv(), Ok(TermEvent::Wakeup)));
    assert!(matches!(events.try_recv(), Ok(TermEvent::PtyWrite(text)) if text == "reply"));
    assert!(
        matches!(events.try_recv(), Ok(TermEvent::ClipboardStore(text)) if text == "clipboard")
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
        terminal.on_event(TermEvent::ClipboardStore("kept".into()), cx);
    });
    cx.run_until_parked();
    assert_eq!(notifications.get(), 0);
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("kept".into())
    );
    terminal.update(cx, |terminal, cx| {
        let text = terminal.fixture_screen();
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
        terminal
            .term
            .lock()
            .unwrap()
            .select(6, 3, true, SelectionKind::Simple, false)
            .unwrap();
        terminal.selecting = true;
        // Holding a click alone must not scroll.
        assert!(terminal.scroll_selection(cx));
        assert_eq!(terminal.term.lock().unwrap().display_offset(), 0);
        terminal.selection_pointer = Some(point(px(0.), px(0.)));
        assert!(terminal.scroll_selection(cx));
        assert_eq!(terminal.term.lock().unwrap().display_offset(), 1);
        let text = terminal.term.lock().unwrap().selection_text().unwrap();
        assert!(text.contains("line-25"), "{text}");
        assert!(text.contains("line-29"), "{text}");
        // A second tick at the same pointer position must keep extending.
        assert!(terminal.scroll_selection(cx));
        assert!(
            terminal
                .term
                .lock()
                .unwrap()
                .selection_text()
                .unwrap()
                .contains("line-24")
        );
        terminal.selection_pointer = Some(point(px(0.), px(32.)));
        terminal.scroll_selection(cx);
        assert_eq!(terminal.term.lock().unwrap().display_offset(), 2);
        // Dragging past the bottom returns toward the live screen.
        terminal.selection_pointer = Some(point(px(0.), px(80.)));
        terminal.scroll_selection(cx);
        assert_eq!(terminal.term.lock().unwrap().display_offset(), 0);
        terminal.stop_selection();
        assert!(!terminal.scroll_selection(cx));
    });
}

#[test]
fn keys_encode_like_xterm_with_the_macos_editing_shortcuts() {
    assert_eq!(bytes("enter"), Some(b"\r".to_vec()));
    assert_eq!(bytes("backspace"), Some(b"\x7f".to_vec()));
    assert_eq!(bytes("up"), Some(b"\x1b[A".to_vec()));
    let app_cursor = engine(GridSize {
        columns: 80,
        lines: 24,
    });
    app_cursor.lock().unwrap().write(b"\x1b[?1h");
    assert_eq!(
        key_bytes(&key("up"), &mut app_cursor.lock().unwrap()),
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
    assert_eq!(
        key_bytes(&typed, &mut app_cursor.lock().unwrap()),
        Some(b"a".to_vec())
    );
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
    let term = engine(GridSize {
        columns: 20,
        lines: 4,
    });
    let mut term = term.lock().unwrap();
    term.write(b"\x1b]11;?\x07\x1b]12;?\x07");
    let replies = term
        .take_effects()
        .into_iter()
        .filter_map(|effect| match effect {
            vorkspace_terminal::Effect::PtyWrite(bytes) => {
                Some(String::from_utf8_lossy(&bytes).into_owned())
            }
            _ => None,
        })
        .collect::<String>();
    assert!(replies.contains("rgb:1717/1717/1717"), "{replies:?}");
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
    let screen = || spawned.term.lock().unwrap().frame().unwrap().text();
    let mut seen = false;
    for _ in 0..250 {
        std::thread::sleep(Duration::from_millis(20));
        if screen().contains("bot-code") {
            seen = true;
            break;
        }
    }
    assert!(seen, "{}", screen());
    assert_eq!(spawned.term.lock().unwrap().frame().unwrap().columns, 60);
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

fn command_pty(
    command: &str,
    size: GridSize,
) -> (
    pty::Pty,
    Arc<Mutex<Engine>>,
    async_channel::Receiver<TermEvent>,
) {
    let term = engine(size);
    let (listener, events) = Listener::channel();
    let pty = pty::Pty::spawn(
        "/bin/sh",
        &["-c".into(), command.into()],
        &std::env::temp_dir(),
        HashMap::new(),
        size,
        term.clone(),
        listener,
    )
    .unwrap();
    (pty, term, events)
}
fn wait_for_exit(events: &async_channel::Receiver<TermEvent>) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        while let Ok(event) = events.try_recv() {
            if let TermEvent::ChildExit(status) = event {
                return status;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("PTY child did not exit");
}

#[test]
fn pty_drains_final_output_before_publishing_child_exit() {
    let (_pty, term, events) = command_pty(
        "printf '\\033[31mfinal-output\\033[0m'; exit 7",
        GridSize {
            columns: 40,
            lines: 4,
        },
    );
    assert_eq!(wait_for_exit(&events).code(), Some(7));
    let frame = term.lock().unwrap().frame().unwrap();
    assert!(frame.text().contains("final-output"));
    assert_eq!(frame.rows[0][0].foreground, ANSI[1]);
}

#[test]
fn pty_resize_precedes_subsequent_input() {
    let size = GridSize {
        columns: 40,
        lines: 4,
    };
    let (pty, term, events) = command_pty("read ignored; stty size", size);
    let next = GridSize {
        columns: 42,
        lines: 7,
    };
    term.lock().unwrap().resize(42, 7, 8, 16).unwrap();
    pty.sender
        .send(Msg::Resize {
            size: next,
            cell_width: 8,
            cell_height: 16,
        })
        .unwrap();
    pty.sender
        .send(Msg::Input(Cow::Borrowed(b"continue\r")))
        .unwrap();
    assert!(wait_for_exit(&events).success());
    assert!(
        term.lock()
            .unwrap()
            .frame()
            .unwrap()
            .text()
            .contains("7 42")
    );
}

#[test]
fn pty_close_interrupts_a_full_input_queue_and_reaps_an_uncooperative_shell() {
    use std::os::unix::process::ExitStatusExt;
    let (pty, term, events) = command_pty(
        "trap '' HUP; printf ready; while :; do :; done",
        GridSize {
            columns: 40,
            lines: 4,
        },
    );
    // The marker establishes that the child installed its hangup policy.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut ready = false;
    while std::time::Instant::now() < deadline {
        if matches!(events.try_recv(), Ok(TermEvent::Wakeup)) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready, "shell did not become ready");
    pty.sender
        .send(Msg::Input(Cow::Owned(vec![b'x'; 4 * 1024 * 1024])))
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut echo_seen = false;
    while std::time::Instant::now() < deadline {
        if term.lock().unwrap().frame().unwrap().text().contains("xxx") {
            echo_seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        echo_seen,
        "the PTY accepted and echoed part of the large paste"
    );
    pty.sender.send(Msg::Shutdown).unwrap();
    assert_eq!(wait_for_exit(&events).signal(), Some(libc::SIGKILL));
    assert_eq!(
        unsafe { libc::kill(pty.shell_pid as i32, 0) },
        -1,
        "the child was reaped"
    );
}

#[test]
fn closing_the_login_shell_also_kills_an_attached_job_that_ignores_hangup() {
    use std::os::fd::{AsRawFd, FromRawFd};
    let size = GridSize {
        columns: 120,
        lines: 6,
    };
    let term = engine(size);
    let (listener, events) = Listener::channel();
    let pty = pty::Pty::spawn(
        "/bin/sh",
        &["-i".into()],
        &std::env::temp_dir(),
        HashMap::new(),
        size,
        term.clone(),
        listener,
    )
    .unwrap();
    pty.sender
        .send(Msg::Input(Cow::Borrowed(
            b"/bin/sh -c 'trap \"\" HUP; printf \"job%sready\" -; while :; do :; done'\r",
        )))
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut ready = false;
    while std::time::Instant::now() < deadline {
        if term
            .lock()
            .unwrap()
            .frame()
            .unwrap()
            .text()
            .contains("job-ready")
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready, "the foreground job installed its hangup policy");
    let group = unsafe { libc::tcgetpgrp(pty.master.as_raw_fd()) };
    assert!(group > 1 && group != pty.shell_pid as i32);
    assert_eq!(unsafe { libc::getsid(group) }, pty.shell_pid as i32);
    let queue = unsafe { libc::kqueue() };
    assert!(queue >= 0);
    let queue = unsafe { std::fs::File::from_raw_fd(queue) };
    let change = libc::kevent {
        ident: group as usize,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    assert_eq!(
        unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        },
        0
    );
    pty.sender.send(Msg::Shutdown).unwrap();
    let _ = wait_for_exit(&events);
    let mut exited: libc::kevent = unsafe { std::mem::zeroed() };
    let timeout = libc::timespec {
        tv_sec: 5,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                std::ptr::null(),
                0,
                &mut exited,
                1,
                &timeout,
            )
        },
        1,
        "the attached foreground group must exit too"
    );
    let exited_pid = exited.ident;
    assert_eq!(exited_pid, group as usize);
    assert!(exited.fflags & libc::NOTE_EXIT != 0);
}
