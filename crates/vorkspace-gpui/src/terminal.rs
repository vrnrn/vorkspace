//! A login shell on an owned PTY, emulated by the public libghostty-vt API
//! and painted by GPUI as a grid of cell-snapped grapheme runs.
mod pty;
use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, Font, FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId, KeyBinding,
    KeyDownEvent, KeyUpEvent, Keystroke, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    ScrollDelta, ScrollWheelEvent, ShapedLine, SharedString, Size, StrikethroughStyle, Style, Task,
    TextAlign, TextRun, UnderlineStyle, Window, actions, div, point, prelude::*, px, rgb, size,
};
use pty::{Msg, Sender as EventLoopSender};
use std::{
    borrow::Cow,
    cell::Cell,
    collections::HashMap,
    os::fd::AsRawFd,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use vorkspace_terminal::{
    Cell as TermCell, Config, CursorShape, Key, KeyAction, Modes as TermMode, SelectionKind,
    Terminal as Engine,
};
use vorkspace_ui::theme::*;

actions!(
    terminal,
    [SendTab, SendBackTab, SendEscape, Copy, Paste, SelectAll]
);

/// A worktree being removed can't host a new shell.
pub(crate) fn usable_cwd(cwd: String) -> String {
    if vorkspace_core::worktree_management::is_being_removed(&cwd) {
        std::env::var("HOME").unwrap_or(cwd)
    } else {
        cwd
    }
}
pub(crate) fn bind(cx: &mut App) {
    let m = "cmd";
    let c = Some("Terminal");
    // The shell owns these keys while a terminal has focus.
    cx.bind_keys([
        KeyBinding::new("tab", SendTab, c),
        KeyBinding::new("shift-tab", SendBackTab, c),
        KeyBinding::new("escape", SendEscape, c),
        KeyBinding::new(&format!("{m}-c"), Copy, c),
        KeyBinding::new(&format!("{m}-v"), Paste, c),
        KeyBinding::new(&format!("{m}-a"), SelectAll, c),
    ]);
}

const FONT_SIZE: f32 = 13.;
/// TerminalView's `scrollback`, retained for every open terminal.
const SCROLLBACK: usize = 2000;
const PADDING: f32 = 8.;

/// Wakeups, titles and replies from the PTY thread.
#[derive(Clone)]
struct Listener {
    events: async_channel::Sender<TermEvent>,
    repaint: Arc<Repaint>,
}

struct Repaint {
    visible: AtomicBool,
    pending: AtomicBool,
}
impl Listener {
    fn channel() -> (Self, async_channel::Receiver<TermEvent>) {
        let (events, receiver) = async_channel::unbounded();
        (
            Self {
                events,
                repaint: Arc::new(Repaint {
                    visible: AtomicBool::new(true),
                    pending: AtomicBool::new(false),
                }),
            },
            receiver,
        )
    }
}
impl Listener {
    fn send_event(&self, event: TermEvent) {
        let event = match event {
            TermEvent::Wakeup => {
                // The terminal grid already owns the latest output. Queue
                // one request to observe it, regardless of output rate.
                if !self.repaint.visible.load(Ordering::Acquire)
                    || self.repaint.pending.swap(true, Ordering::AcqRel)
                {
                    return;
                }
                TermEvent::Wakeup
            }
            event => event,
        };
        let _ = self.events.try_send(event);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GridSize {
    columns: usize,
    lines: usize,
}
enum TermEvent {
    Wakeup,
    #[cfg(test)]
    PtyWrite(String),
    ClipboardStore(String),
    ChildExit(std::process::ExitStatus),
    Exit,
}

pub(crate) enum TerminalEvent {
    /// Title, foreground process or running state changed.
    Changed,
}

/// Where the grid was painted, for mapping the pointer to cells.
#[derive(Clone, Copy, Default)]
struct Geometry {
    origin: gpui::Point<gpui::Pixels>,
    cell: Size<gpui::Pixels>,
}

pub(crate) struct Terminal {
    term: Arc<Mutex<Engine>>,
    sender: Option<EventLoopSender>,
    focus: FocusHandle,
    pub cwd: String,
    /// The tab label: the foreground command, else the directory.
    pub title: String,
    /// A command other than the shell holds the terminal.
    pub foreground: Option<String>,
    pub exited: bool,
    size: GridSize,
    geometry: Rc<Cell<Option<Geometry>>>,
    master_fd: i32,
    master_owner: Option<Arc<std::fs::File>>,
    shell_pid: u32,
    selecting: bool,
    selection_pointer: Option<gpui::Point<gpui::Pixels>>,
    selection_scroll: Option<Task<()>>,
    scroll_remainder: f32,
    repaint: Arc<Repaint>,
    _events: Task<()>,
    foreground_poll: Option<Task<()>>,
}
impl EventEmitter<TerminalEvent> for Terminal {}
impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

struct Spawned {
    term: Arc<Mutex<Engine>>,
    sender: Option<EventLoopSender>,
    master_fd: i32,
    master_owner: Option<Arc<std::fs::File>>,
    shell_pid: u32,
    events: async_channel::Receiver<TermEvent>,
    repaint: Arc<Repaint>,
}
/// Start the login shell on a PTY with TerminalView's environment. A failure
/// is written into the grid, as TerminalView prints it.
fn engine(size: GridSize) -> Arc<Mutex<Engine>> {
    let config = Config {
        scrollback_lines: SCROLLBACK,
        scrollback_bytes: 8 * 1024 * 1024,
        palette: std::array::from_fn(|i| indexed_rgb(i as u8)),
        foreground: OSC_FOREGROUND,
        background: OSC_BACKGROUND,
        cursor: ACCENT,
    };
    Arc::new(Mutex::new(
        Engine::new(size.columns as u16, size.lines as u16, &config)
            .expect("allocate libghostty-vt terminal"),
    ))
}
fn spawn(cwd: &str, size: GridSize) -> Spawned {
    let (listener, rx) = Listener::channel();
    let repaint = listener.repaint.clone();
    let term = engine(size);
    let workdir = working_dir(cwd);
    let (program, args) = default_shell();
    let mut env = HashMap::new();
    for (key, value) in [
        ("TERM", "xterm-256color".to_owned()),
        ("COLORTERM", "truecolor".to_owned()),
        ("COLORFGBG", "15;0".to_owned()),
        ("TERM_PROGRAM", "vorkspace".to_owned()),
        ("PATH", vorkspace_core::scm::terminal_search_path()),
        ("PWD", workdir.to_string_lossy().into_owned()),
    ] {
        env.insert(key.to_owned(), value);
    }
    let (sender, master_owner, master_fd, shell_pid) =
        match pty::Pty::spawn(&program, &args, &workdir, env, size, term.clone(), listener) {
            Ok(pty) => {
                let fd = pty.master.as_raw_fd();
                (Some(pty.sender), Some(pty.master), fd, pty.shell_pid)
            }
            Err(error) => {
                write_note(
                    &term,
                    &format!("\x1b[31mCould not start the shell: {error}\x1b[0m"),
                );
                (None, None, -1, 0)
            }
        };
    Spawned {
        term,
        sender,
        master_fd,
        master_owner,
        shell_pid,
        events: rx,
        repaint,
    }
}

fn detached(size: GridSize) -> Spawned {
    let (listener, rx) = Listener::channel();
    Spawned {
        term: engine(size),
        sender: None,
        master_fd: -1,
        master_owner: None,
        shell_pid: 0,
        events: rx,
        repaint: listener.repaint,
    }
}

impl Terminal {
    #[cfg(test)]
    pub(crate) fn fixture_output(&mut self, text: &str, cx: &mut Context<Self>) {
        write_note(&self.term, text);
        self.on_event(TermEvent::Wakeup, cx);
    }

    #[cfg(test)]
    pub(crate) fn fixture_screen(&self) -> String {
        self.term.lock().unwrap().frame().unwrap().text()
    }

    /// Hidden terminals do not poll. Query at the explicit close boundary so
    /// an old title cannot hide a job started before switching tabs.
    pub(crate) fn foreground_process(&self) -> Option<String> {
        if cfg!(test) {
            return self.foreground.clone();
        }
        (!self.exited)
            .then(|| foreground_label(self.master_fd, self.shell_pid))
            .flatten()
    }

    pub(crate) fn new(cwd: String, cx: &mut Context<Self>) -> Self {
        let size = GridSize {
            columns: 80,
            lines: 24,
        };
        let Spawned {
            term,
            sender,
            master_fd,
            master_owner,
            shell_pid,
            events: rx,
            repaint,
        } = if cfg!(test) {
            // GPUI's deterministic test scheduler rejects wakeups from the
            // PTY thread; UI tests run the dock without a shell behind it.
            detached(size)
        } else {
            spawn(&cwd, size)
        };
        let events = cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                // Output arrives in bursts; one repaint covers the burst.
                let mut batch = vec![event];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                if this
                    .update(cx, |this, cx| {
                        for event in batch {
                            this.on_event(event, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut terminal = Self {
            term,
            sender,
            focus: cx.focus_handle(),
            title: default_title(&cwd),
            cwd,
            foreground: None,
            exited: false,
            size,
            geometry: Rc::default(),
            master_fd,
            master_owner,
            shell_pid,
            selecting: false,
            selection_pointer: None,
            selection_scroll: None,
            scroll_remainder: 0.,
            repaint,
            _events: events,
            foreground_poll: None,
        };
        terminal.start_foreground_poll(cx);
        terminal
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.repaint.visible.load(Ordering::Acquire)
    }

    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.set_activity(visible, visible, cx);
    }

    /// A dock can show foreground status even when the grid is folded away.
    pub(crate) fn set_activity(
        &mut self,
        visible: bool,
        monitor_foreground: bool,
        cx: &mut Context<Self>,
    ) {
        if monitor_foreground {
            self.start_foreground_poll(cx);
        } else {
            self.foreground_poll = None;
        }
        if self.is_visible() == visible {
            return;
        }
        self.repaint.visible.store(visible, Ordering::Release);
        if visible {
            cx.notify();
        } else {
            self.stop_selection();
        }
    }

    fn start_foreground_poll(&mut self, cx: &mut Context<Self>) {
        if self.foreground_poll.is_some() || self.exited {
            return;
        }
        let Some(master) = self.master_owner.clone() else {
            return;
        };
        let pid = self.shell_pid;
        self.foreground_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let master = master.clone();
                let label = cx
                    .background_executor()
                    .spawn(async move { foreground_label(master.as_raw_fd(), pid) })
                    .await;
                if this
                    .update(cx, |this, cx| this.set_foreground(label, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn on_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::Wakeup => {
                // Output during painting must be able to queue the next wakeup.
                self.repaint.pending.store(false, Ordering::Release);
                if self.is_visible() {
                    cx.notify();
                }
            }
            #[cfg(test)]
            TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
            TermEvent::ClipboardStore(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            TermEvent::ChildExit(status) => {
                let code = status.code().map(|c| format!(" ({c})")).unwrap_or_default();
                write_note(&self.term, &format!("\r\n[process exited{code}]"));
                self.finish(cx);
            }
            TermEvent::Exit => self.finish(cx),
        }
    }
    fn finish(&mut self, cx: &mut Context<Self>) {
        if !self.exited {
            self.exited = true;
            self.sender = None;
            self.foreground_poll = None;
            self.set_foreground(None, cx);
            cx.emit(TerminalEvent::Changed);
        }
        cx.notify();
    }
    fn set_foreground(&mut self, label: Option<String>, cx: &mut Context<Self>) {
        if label == self.foreground {
            return;
        }
        self.title = label.clone().unwrap_or_else(|| default_title(&self.cwd));
        self.foreground = label;
        cx.emit(TerminalEvent::Changed);
    }
    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if let Some(sender) = &self.sender
            && let Err(error) = sender.send(Msg::Input(bytes.into()))
            && error.kind() == std::io::ErrorKind::WouldBlock
        {
            write_note(&self.term, "\r\n[terminal input queue is full]");
        }
    }
    /// Typing returns a scrolled-back view to the prompt.
    fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.stop_selection();
        {
            let mut term = self.term.lock().unwrap();
            term.scroll_to_bottom();
            term.clear_selection();
        }
        self.write(bytes);
        cx.notify();
    }
    /// Fit the grid to the painted area; the PTY learns its new size.
    fn resize(&mut self, columns: usize, lines: usize, cell: Size<gpui::Pixels>) {
        let next = GridSize { columns, lines };
        if next == self.size && self.geometry.get().is_some_and(|g| g.cell == cell) {
            return;
        }
        let effects = {
            let mut term = self.term.lock().unwrap();
            if let Err(error) = term.resize(
                columns as u16,
                lines as u16,
                cell.width.as_f32().max(1.) as u32,
                cell.height.as_f32().max(1.) as u32,
            ) {
                log_terminal_error(&error);
                return;
            }
            term.take_effects()
        };
        self.size = next;
        for effect in effects {
            if let vorkspace_terminal::Effect::PtyWrite(bytes) = effect {
                self.write(bytes);
            }
        }
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Resize {
                size: next,
                cell_width: cell.width.as_f32() as u16,
                cell_height: cell.height.as_f32() as u16,
            });
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let bytes = encode_key(
            &event.keystroke,
            &mut self.term.lock().unwrap(),
            if event.is_held {
                KeyAction::Repeat
            } else {
                KeyAction::Press
            },
        );
        if let Some(bytes) = bytes {
            cx.stop_propagation();
            self.input(bytes, cx);
        }
    }
    fn key_up(&mut self, event: &KeyUpEvent, cx: &mut Context<Self>) {
        let bytes = encode_key(
            &event.keystroke,
            &mut self.term.lock().unwrap(),
            KeyAction::Release,
        );
        if let Some(bytes) = bytes {
            cx.stop_propagation();
            self.write(bytes);
        }
    }
    fn action_key(&mut self, spec: &str, cx: &mut Context<Self>) {
        let key = Keystroke::parse(spec).expect("static terminal key");
        let bytes = encode_key(&key, &mut self.term.lock().unwrap(), KeyAction::Press);
        if let Some(bytes) = bytes {
            self.input(bytes, cx);
        }
    }
    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self
            .term
            .lock()
            .unwrap()
            .selection_text()
            .filter(|t| !t.is_empty())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self
            .term
            .lock()
            .unwrap()
            .modes()
            .contains(TermMode::BRACKETED_PASTE);
        let bytes = if bracketed {
            // A pasted end marker must not end the paste early.
            let text = text.replace("\x1b[201~", "");
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.input(bytes, cx);
    }
    fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.term.lock().unwrap().select_all() {
            log_terminal_error(&error);
        }
        cx.notify();
    }

    /// The cell under a window position, and which half of it.
    fn grid_point(&self, position: gpui::Point<gpui::Pixels>) -> Option<(u16, u16, bool)> {
        let g = self.geometry.get()?;
        let x = ((position.x - g.origin.x) / g.cell.width).max(0.);
        let y = ((position.y - g.origin.y) / g.cell.height).max(0.);
        let column = (x as usize).min(self.size.columns.saturating_sub(1));
        let line = (y as usize).min(self.size.lines.saturating_sub(1));
        Some((column as u16, line as u16, x.fract() >= 0.5))
    }
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((column, row, right)) = self.grid_point(event.position) else {
            return;
        };
        let kind = match event.click_count {
            2 => SelectionKind::Word,
            3.. => SelectionKind::Line,
            _ => SelectionKind::Simple,
        };
        if let Err(error) =
            self.term
                .lock()
                .unwrap()
                .select(column, row, right, kind, event.modifiers.shift)
        {
            log_terminal_error(&error);
        }
        self.selecting = true;
        self.selection_pointer = None;
        self.selection_scroll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if !this
                    .update_in(cx, |this, window, cx| {
                        if !window.is_window_active() || !this.focus.is_focused(window) {
                            this.selecting = false;
                            this.selection_pointer = None;
                            return false;
                        }
                        this.scroll_selection(cx)
                    })
                    .unwrap_or(false)
                {
                    break;
                }
            }
        }));
        cx.notify();
    }
    fn stop_selection(&mut self) {
        self.selecting = false;
        self.selection_pointer = None;
        self.selection_scroll = None;
    }
    /// Keep extending the same selection through scrollback at either edge,
    /// even when the pointer is held still outside the terminal.
    fn scroll_selection(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.selecting || !self.is_visible() {
            self.selecting = false;
            self.selection_pointer = None;
            return false;
        }
        let Some(position) = self.selection_pointer else {
            // A press without a drag must not scroll a first/last-row click.
            return true;
        };
        let Some(geometry) = self.geometry.get() else {
            return true;
        };
        let y = (position.y - geometry.origin.y) / geometry.cell.height;
        let delta = if y < 0.5 {
            (0.5 - y).ceil().clamp(1., 10.) as i32
        } else if y > self.size.lines as f32 - 0.5 {
            -((y - self.size.lines as f32 + 0.5).ceil().clamp(1., 10.) as i32)
        } else {
            return true;
        };
        let changed = {
            let mut term = self.term.lock().unwrap();
            let before = term.display_offset();
            term.scroll(delta);
            term.display_offset() != before
        };
        if changed {
            if let Some((column, row, right)) = self.grid_point(position) {
                let _ = self.term.lock().unwrap().select(
                    column,
                    row,
                    right,
                    SelectionKind::Simple,
                    true,
                );
            }
            cx.notify();
        }
        true
    }
    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.stop_selection();
            return;
        }
        self.selection_pointer = Some(event.position);
        if let Some((column, row, right)) = self.grid_point(event.position) {
            let _ =
                self.term
                    .lock()
                    .unwrap()
                    .select(column, row, right, SelectionKind::Simple, true);
            cx.notify();
        }
    }
    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let line = self.geometry.get().map_or(16., |g| g.cell.height.as_f32());
        let delta = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => pixels.y.as_f32() / line,
        } + self.scroll_remainder;
        let lines = delta.trunc() as i32;
        self.scroll_remainder = delta.fract();
        if lines == 0 {
            return;
        }
        let mode = self.term.lock().unwrap().modes();
        // A full-screen program scrolls itself; the shell scrolls history.
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let key: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.write(key.repeat(lines.unsigned_abs() as usize));
        } else {
            self.term.lock().unwrap().scroll(lines);
            cx.notify();
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Msg::Shutdown);
        }
    }
}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .cursor(gpui::CursorStyle::IBeam)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .on_key_up(cx.listener(|this, event, _, cx| this.key_up(event, cx)))
            .on_action(cx.listener(|this, _: &SendTab, _, cx| this.action_key("tab", cx)))
            .on_action(cx.listener(|this, _: &SendBackTab, _, cx| this.action_key("shift-tab", cx)))
            .on_action(cx.listener(|this, _: &SendEscape, _, cx| this.action_key("escape", cx)))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.mouse_down(event, window, cx)),
            )
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.scroll(event, cx)))
            .child(Grid {
                terminal: cx.entity(),
                focused,
            })
    }
}

/// One shaped run at a column, or a filled cell span.
struct Painted {
    lines: Vec<(gpui::Point<gpui::Pixels>, ShapedLine)>,
    rects: Vec<(Bounds<gpui::Pixels>, Hsla)>,
    cursor: Option<(Bounds<gpui::Pixels>, Hsla, bool)>,
    line_height: gpui::Pixels,
}

struct Grid {
    terminal: Entity<Terminal>,
    focused: bool,
}
impl IntoElement for Grid {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
/// A cell to paint: column, glyph, pen, fill and whether it is wide.
type Painting = (usize, vorkspace_terminal::Text, Pen, Option<Hsla>, bool);
#[derive(Clone, PartialEq)]
struct Pen {
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}
impl Element for Grid {
    type RequestLayoutState = ();
    type PrepaintState = Painted;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = gpui::relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<gpui::Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Painted {
        let font_size = px(FONT_SIZE);
        let base = fixed_font();
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&base);
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map_or(px(7.8), |size| size.width);
        let line_height = (text_system.ascent(font_id, font_size)
            + text_system.descent(font_id, font_size).abs())
        .ceil()
        .max(px(14.));
        let origin = point(bounds.left() + px(PADDING), bounds.top() + px(PADDING / 2.));
        let columns =
            (((bounds.size.width - px(PADDING * 2.)) / cell_width).floor() as usize).max(2);
        let lines = (((bounds.size.height - px(PADDING)) / line_height).floor() as usize).max(1);
        let cell = size(cell_width, line_height);
        let (term, geometry) = self.terminal.update(cx, |t, _| {
            t.resize(columns, lines, cell);
            (t.term.clone(), t.geometry.clone())
        });
        geometry.set(Some(Geometry { origin, cell }));

        let frame = {
            let mut term = term.lock().unwrap();
            let _ = term.set_default_colors(
                color_hex(content(1.)),
                color_hex(pane_fill()),
                color_hex(accent_color()),
            );
            term.frame()
        }
        .unwrap_or_else(|error| {
            log_terminal_error(&error);
            vorkspace_terminal::Frame::default()
        });
        let mut rows: Vec<Vec<Painting>> = vec![Vec::new(); lines];
        for (row, cells) in frame.rows.iter().enumerate().take(lines) {
            for cell in cells {
                let mut fg: Hsla = rgb(cell.foreground).into();
                let mut bg = cell.background_set.then(|| rgb(cell.background).into());
                if cell.has(TermCell::INVERSE) {
                    let back = bg.unwrap_or(rgb(frame.background).into());
                    bg = Some(fg);
                    fg = back;
                }
                if cell.has(TermCell::DIM) {
                    fg.a *= 0.66;
                }
                if cell.selected {
                    bg = Some(selection_fill());
                }
                let text = if cell.has(TermCell::HIDDEN) || cell.text.is_empty() {
                    " ".into()
                } else {
                    cell.text.clone()
                };
                rows[row].push((
                    cell.column,
                    text,
                    Pen {
                        fg,
                        bold: cell.has(TermCell::BOLD),
                        italic: cell.has(TermCell::ITALIC),
                        underline: cell.has(TermCell::UNDERLINE),
                        strike: cell.has(TermCell::STRIKE),
                    },
                    bg,
                    cell.width == 2,
                ));
            }
        }
        let cursor = frame.cursor;

        let mut painted = Painted {
            lines: Vec::new(),
            rects: if frame.background != color_hex(pane_fill()) {
                vec![(bounds, rgb(frame.background).into())]
            } else {
                Vec::new()
            },
            cursor: None,
            line_height,
        };
        for (line, cells) in rows.iter().enumerate() {
            let y = origin.y + line_height * line as f32;
            // Backgrounds merge across equal neighbours.
            let mut span: Option<(usize, usize, Hsla)> = None;
            for (column, _, _, bg, wide) in cells {
                let width = if *wide { 2 } else { 1 };
                match (span, bg) {
                    (Some((start, end, fill)), Some(bg)) if end == *column && fill == *bg => {
                        span = Some((start, end + width, fill));
                    }
                    (_, bg) => {
                        if let Some((start, end, fill)) = span.take() {
                            painted
                                .rects
                                .push((cell_rect(origin, cell, line, start, end), fill));
                        }
                        span = bg.map(|bg| (*column, column + width, bg));
                    }
                }
            }
            if let Some((start, end, fill)) = span {
                painted
                    .rects
                    .push((cell_rect(origin, cell, line, start, end), fill));
            }
            // Text runs break where the pen changes, and around wide glyphs
            // so each one spans its two cells.
            let mut ix = 0;
            while ix < cells.len() {
                let (column, _, pen, _, wide) = &cells[ix];
                let mut text = String::new();
                let mut end = ix;
                while end < cells.len()
                    && cells[end].2 == *pen
                    && !cells[end].4
                    && (end == ix || !wide)
                    && cells[end].0 == column + (end - ix)
                {
                    text.push_str(&cells[end].1);
                    end += 1;
                }
                if end == ix {
                    text.push_str(&cells[ix].1);
                    end = ix + 1;
                }
                if !text.trim().is_empty() {
                    let mut font = base.clone();
                    if pen.bold {
                        font.weight = FontWeight::BOLD;
                    }
                    if pen.italic {
                        font.style = FontStyle::Italic;
                    }
                    let run = TextRun {
                        len: text.len(),
                        font,
                        color: pen.fg,
                        background_color: None,
                        underline: pen.underline.then_some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(pen.fg),
                            wavy: false,
                        }),
                        strikethrough: pen.strike.then_some(StrikethroughStyle {
                            thickness: px(1.),
                            color: Some(pen.fg),
                        }),
                    };
                    let width = if *wide { cell_width * 2. } else { cell_width };
                    let shaped = text_system.shape_line(
                        SharedString::from(text),
                        font_size,
                        &[run],
                        Some(width),
                    );
                    painted
                        .lines
                        .push((point(origin.x + cell_width * *column as f32, y), shaped));
                }
                ix = end;
            }
        }
        if let Some(cursor) = cursor.filter(|cursor| cursor.row < lines && cursor.column < columns)
        {
            let cursor_shape = cursor.shape;
            let x = origin.x + cell_width * cursor.column as f32;
            let y = origin.y + line_height * cursor.row as f32;
            let accent: Hsla = rgb(cursor.color).into();
            let (bounds, hollow) = match (cursor_shape, self.focused) {
                (CursorShape::Beam, _) => {
                    (Bounds::new(point(x, y), size(px(2.), line_height)), false)
                }
                (CursorShape::Underline, _) => (
                    Bounds::new(point(x, y + line_height - px(2.)), size(cell_width, px(2.))),
                    false,
                ),
                (CursorShape::Block, true) => (Bounds::new(point(x, y), cell), false),
                _ => (Bounds::new(point(x, y), cell), true),
            };
            let mut fill = accent;
            if !self.focused {
                fill.a = 0.5;
            } else if cursor_shape == CursorShape::Block {
                fill.a = 0.6;
            }
            painted.cursor = Some((bounds, fill, hollow));
        }
        painted
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<gpui::Pixels>,
        _: &mut (),
        painted: &mut Painted,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Element mouse handlers only receive hovered events. An active drag
        // must keep tracking movement and release anywhere in the window.
        let terminal = self.terminal.downgrade();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == gpui::DispatchPhase::Capture {
                let _ = terminal.update(cx, |terminal, cx| terminal.mouse_move(event, cx));
            }
        });
        let terminal = self.terminal.downgrade();
        window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
            if phase == gpui::DispatchPhase::Capture && event.button == MouseButton::Left {
                let _ = terminal.update(cx, |terminal, _| terminal.stop_selection());
            }
        });
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for (rect, fill) in &painted.rects {
                window.paint_quad(gpui::fill(*rect, *fill));
            }
            for (origin, line) in &painted.lines {
                let _ = line.paint(
                    *origin,
                    painted.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            if let Some((rect, fill, hollow)) = painted.cursor {
                if hollow {
                    window.paint_quad(gpui::outline(rect, fill, gpui::BorderStyle::Solid));
                } else {
                    window.paint_quad(gpui::fill(rect, fill));
                }
            }
        });
    }
}

fn cell_rect(
    origin: gpui::Point<gpui::Pixels>,
    cell: Size<gpui::Pixels>,
    line: usize,
    start: usize,
    end: usize,
) -> Bounds<gpui::Pixels> {
    Bounds::new(
        point(
            origin.x + cell.width * start as f32,
            origin.y + cell.height * line as f32,
        ),
        size(cell.width * (end - start) as f32, cell.height),
    )
}

fn color_hex(color: Hsla) -> u32 {
    let c = color.to_rgb();
    ((c.r * 255.).round() as u32) << 16
        | ((c.g * 255.).round() as u32) << 8
        | (c.b * 255.).round() as u32
}
fn fixed_font() -> Font {
    gpui::font(vorkspace_ui::fonts::FIXED_FONT)
}
fn selection_fill() -> Hsla {
    content(0.18)
}
fn write_note(term: &Arc<Mutex<Engine>>, text: &str) {
    term.lock().unwrap().write(text.as_bytes());
}
fn log_terminal_error(error: &std::io::Error) {
    eprintln!("Terminal: {error}");
}

/// TerminalView's ANSI_DARK palette.
const ANSI: [u32; 16] = [
    0x1d2428, 0xf87171, 0x4ade80, 0xfbbf24, 0x60a5fa, 0xc084fc, 0x22d3ee, 0xe8eef2, 0x64748b,
    0xfca5a5, 0x86efac, 0xfde68a, 0x93c5fd, 0xd8b4fe, 0x67e8f9, 0xf8fafc,
];
/// oscColors(): what a CLI asking for colours is told.
const OSC_FOREGROUND: u32 = TEXT;
const OSC_BACKGROUND: u32 = 0x171717;

fn indexed_rgb(index: u8) -> u32 {
    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
        }
        _ => {
            let v = 8 + (index as u32 - 232) * 10;
            (v << 16) | (v << 8) | v
        }
    }
}
/// Bytes a keystroke sends, or None to leave it to the app's bindings.
/// Includes terminalKeys.ts's macOS editing shortcuts.
#[cfg(test)]
fn key_bytes(keystroke: &Keystroke, term: &mut Engine) -> Option<Vec<u8>> {
    encode_key(keystroke, term, KeyAction::Press)
}
fn encode_key(keystroke: &Keystroke, term: &mut Engine, action: KeyAction) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    let key = keystroke.key.as_str();
    if m.platform {
        if matches!(action, KeyAction::Release) {
            return None;
        }
        if m.control || m.shift || m.alt {
            return None;
        }
        return match key {
            "left" => Some(b"\x01".to_vec()),
            "right" => Some(b"\x05".to_vec()),
            "backspace" => Some(b"\x15".to_vec()),
            _ => None,
        };
    }
    if m.alt && !m.control && !m.shift {
        if matches!(action, KeyAction::Release) && matches!(key, "left" | "right") {
            return None;
        }
        match key {
            "left" => return Some(b"\x1bb".to_vec()),
            "right" => return Some(b"\x1bf".to_vec()),
            _ => {}
        }
    }
    let fallback = if key == "space" {
        " ".to_owned()
    } else if key.chars().count() == 1 {
        if m.shift {
            key.to_uppercase()
        } else {
            key.to_owned()
        }
    } else {
        String::new()
    };
    let text = if m.alt {
        fallback.as_str()
    } else {
        keystroke.key_char.as_deref().unwrap_or(&fallback)
    };
    let text = if text
        .chars()
        .any(|ch| ch <= '\u{1f}' || ch == '\u{7f}' || ('\u{f700}'..='\u{f8ff}').contains(&ch))
    {
        fallback.as_str()
    } else {
        text
    };
    let codepoint = if key == "space" {
        ' ' as u32
    } else if key.chars().count() == 1 {
        key.chars().next().unwrap() as u32
    } else {
        0
    };
    term.key(Key {
        name: key,
        text,
        unshifted_codepoint: codepoint,
        shift: m.shift,
        control: m.control,
        alt: m.alt,
        action,
    })
    .map_err(|error| log_terminal_error(&error))
    .ok()
    .filter(|bytes| !bytes.is_empty())
}

fn default_title(cwd: &str) -> String {
    let name = std::path::Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.is_empty() || name == "/" {
        "Terminal".into()
    } else {
        name
    }
}
fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
fn working_dir(cwd: &str) -> std::path::PathBuf {
    let path = match cwd.strip_prefix("~") {
        Some(rest) => home().map_or_else(|| cwd.into(), |h| h.join(rest.trim_start_matches('/'))),
        None => std::path::PathBuf::from(cwd),
    };
    if path.is_dir() {
        path
    } else {
        home().unwrap_or(path)
    }
}
fn default_shell() -> (String, Vec<String>) {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/zsh".into());
    let stem = std::path::Path::new(&shell)
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let args = match stem.as_str() {
        "zsh" | "bash" | "sh" | "fish" => vec!["-l".to_owned()],
        _ => Vec::new(),
    };
    (shell, args)
}

/// pty.rs's foreground_label: the command holding the terminal, unless it
/// is the shell itself.
fn foreground_label(master_fd: i32, shell_pid: u32) -> Option<String> {
    if master_fd < 0 {
        return None;
    }
    let pgrp = unsafe { libc::tcgetpgrp(master_fd) };
    if pgrp <= 0 || pgrp == shell_pid as i32 {
        return None;
    }
    let output = std::process::Command::new("ps")
        .args(["-p", &pgrp.to_string(), "-o", "args="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let label = command_label(String::from_utf8_lossy(&output.stdout).trim())?;
    (!is_shell_name(&label)).then_some(label)
}
/// Interpreters name the script they run: `node vite.js` reads `vite.js`.
pub(crate) fn command_label(args: &str) -> Option<String> {
    let mut parts = args.split_whitespace();
    let exe = parts.next()?;
    let base = std::path::Path::new(exe).file_name()?.to_str()?.to_owned();
    let base = base.trim_start_matches('-').to_owned();
    if matches!(
        base.as_str(),
        "node" | "nodejs" | "python" | "python3" | "ruby" | "deno" | "bun"
    ) {
        for part in parts {
            if part.starts_with('-') {
                continue;
            }
            let name = std::path::Path::new(part).file_name()?.to_str()?;
            if !name.starts_with('-') {
                return Some(name.to_owned());
            }
        }
    }
    Some(base)
}
fn is_shell_name(name: &str) -> bool {
    matches!(
        name,
        "zsh" | "bash" | "sh" | "fish" | "nu" | "dash" | "ksh" | "tcsh" | "zsh5"
    )
}

#[cfg(test)]
mod tests;
