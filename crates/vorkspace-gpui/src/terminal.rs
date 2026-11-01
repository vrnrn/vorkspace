//! Port of TerminalView.tsx: a login shell on a PTY, emulated by
//! alacritty_terminal and painted as a grid of cell-snapped text runs.
use alacritty_terminal::{
    event::{Event as TermEvent, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::{Dimensions, Scroll},
    index::{Column, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{
        Config, Term, TermMode, cell::Flags, color::Colors, point_to_viewport, viewport_to_point,
    },
    tty,
    vte::ansi::{Color as AnsiColor, CursorShape, CursorStyle as TermCursorStyle, NamedColor, Rgb},
};
use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, Font, FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId, KeyBinding,
    KeyDownEvent, Keystroke, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, ScrollDelta,
    ScrollWheelEvent, ShapedLine, SharedString, Size, StrikethroughStyle, Style, Task, TextAlign,
    TextRun, UnderlineStyle, Window, actions, div, point, prelude::*, px, rgb, size,
};
use std::{
    borrow::Cow,
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use vorkspace_ui::theme::*;

actions!(
    terminal,
    [SendTab, SendBackTab, SendEscape, Copy, Paste, SelectAll]
);

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
    repaint_pending: Arc<AtomicBool>,
}
impl Listener {
    fn channel() -> (Self, async_channel::Receiver<TermEvent>) {
        let (events, receiver) = async_channel::unbounded();
        (
            Self {
                events,
                repaint_pending: Arc::default(),
            },
            receiver,
        )
    }
}
impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let event = match event {
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {
                // The terminal grid already owns the latest output. Queue
                // one request to observe it, regardless of output rate.
                if self.repaint_pending.swap(true, Ordering::AcqRel) {
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
impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
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
    term: Arc<FairMutex<Term<Listener>>>,
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
    shell_pid: u32,
    selecting: bool,
    selection_pointer: Option<gpui::Point<gpui::Pixels>>,
    selection_scroll: Option<Task<()>>,
    scroll_remainder: f32,
    pub(crate) visible: bool,
    _events: Task<()>,
    _poll: Task<()>,
}
impl EventEmitter<TerminalEvent> for Terminal {}
impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

struct Spawned {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: Option<EventLoopSender>,
    master_fd: i32,
    shell_pid: u32,
    events: async_channel::Receiver<TermEvent>,
    repaint_pending: Arc<AtomicBool>,
}
/// Start the login shell on a PTY with TerminalView's environment. A failure
/// is written into the grid, as TerminalView prints it.
fn spawn(cwd: &str, size: GridSize, id: u64) -> Spawned {
    let (listener, rx) = Listener::channel();
    let repaint_pending = listener.repaint_pending.clone();
    let config = Config {
        scrolling_history: SCROLLBACK,
        default_cursor_style: TermCursorStyle {
            shape: CursorShape::Beam,
            blinking: true,
        },
        ..Config::default()
    };
    let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
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
    let options = tty::Options {
        shell: Some(tty::Shell::new(program, args)),
        working_directory: Some(workdir),
        drain_on_exit: false,
        env,
    };
    let window = WindowSize {
        num_lines: size.lines as u16,
        num_cols: size.columns as u16,
        cell_width: 8,
        cell_height: 16,
    };
    let mut master_fd = -1;
    let mut shell_pid = 0;
    let sender = match tty::new(&options, window, id) {
        Ok(pty) => {
            {
                use std::os::fd::AsRawFd;
                master_fd = pty.file().as_raw_fd();
                shell_pid = pty.child().id();
            }
            match EventLoop::new(term.clone(), listener, pty, false, false) {
                Ok(event_loop) => {
                    let sender = event_loop.channel();
                    event_loop.spawn();
                    Some(sender)
                }
                Err(error) => {
                    write_note(&term, &format!("\x1b[31m{error}\x1b[0m"));
                    None
                }
            }
        }
        Err(error) => {
            write_note(
                &term,
                &format!("\x1b[31mCould not start the shell: {error}\x1b[0m"),
            );
            None
        }
    };
    Spawned {
        term,
        sender,
        master_fd,
        shell_pid,
        events: rx,
        repaint_pending,
    }
}

fn detached(size: GridSize) -> Spawned {
    let (listener, rx) = Listener::channel();
    let repaint_pending = listener.repaint_pending.clone();
    let term = Term::new(Config::default(), &size, listener);
    Spawned {
        term: Arc::new(FairMutex::new(term)),
        sender: None,
        master_fd: -1,
        shell_pid: 0,
        events: rx,
        repaint_pending,
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
        self.term
            .lock()
            .grid()
            .display_iter()
            .map(|cell| cell.c)
            .collect()
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
            shell_pid,
            events: rx,
            repaint_pending,
        } = if cfg!(test) {
            // GPUI's deterministic test scheduler rejects wakeups from the
            // PTY thread; UI tests run the dock without a shell behind it.
            detached(size)
        } else {
            spawn(&cwd, size, cx.entity_id().as_u64())
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
                            if matches!(event, TermEvent::Wakeup) {
                                // Re-arm before observing the grid: output
                                // arriving during paint must get a new wakeup.
                                repaint_pending.store(false, Ordering::Release);
                            }
                            this.on_event(event, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        // TerminalView's 1s foreground poll; a hidden dock has no title to paint.
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(Some((fd, pid))) = this.update(cx, |this, _| {
                    (this.visible && !this.exited).then_some((this.master_fd, this.shell_pid))
                }) else {
                    if this.update(cx, |_, _| ()).is_err() {
                        break;
                    }
                    continue;
                };
                let label = cx
                    .background_executor()
                    .spawn(async move { foreground_label(fd, pid) })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.visible && !this.exited {
                            this.set_foreground(label, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
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
            shell_pid,
            selecting: false,
            selection_pointer: None,
            selection_scroll: None,
            scroll_remainder: 0.,
            visible: true,
            _events: events,
            _poll: poll,
        }
    }

    fn on_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {
                if self.visible {
                    cx.notify();
                }
            }
            TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
            TermEvent::ColorRequest(index, format) => {
                let colors = *self.term.lock().colors();
                self.write(format(color_rgb(index, &colors)).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let cell = self
                    .geometry
                    .get()
                    .map_or(size(px(8.), px(16.)), |g| g.cell);
                let reply = format(WindowSize {
                    num_lines: self.size.lines as u16,
                    num_cols: self.size.columns as u16,
                    cell_width: cell.width.as_f32() as u16,
                    cell_height: cell.height.as_f32() as u16,
                });
                self.write(reply.into_bytes());
            }
            TermEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            TermEvent::ClipboardLoad(_, format) => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                self.write(format(&text).into_bytes());
            }
            TermEvent::ChildExit(status) => {
                let code = status.code().map(|c| format!(" ({c})")).unwrap_or_default();
                write_note(&self.term, &format!("\r\n[process exited{code}]"));
                self.finish(cx);
            }
            TermEvent::Exit => self.finish(cx),
            TermEvent::Title(_) | TermEvent::ResetTitle | TermEvent::Bell => {}
        }
    }
    fn finish(&mut self, cx: &mut Context<Self>) {
        if !self.exited {
            self.exited = true;
            self.sender = None;
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
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Input(bytes.into()));
        }
    }
    /// Typing returns a scrolled-back view to the prompt.
    fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.stop_selection();
        {
            let mut term = self.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.write(bytes);
        cx.notify();
    }
    /// Fit the grid to the painted area; the PTY learns its new size.
    fn resize(&mut self, columns: usize, lines: usize, cell: Size<gpui::Pixels>) {
        let next = GridSize { columns, lines };
        if next == self.size {
            return;
        }
        self.size = next;
        self.term.lock().resize(next);
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Resize(WindowSize {
                num_lines: lines as u16,
                num_cols: columns as u16,
                cell_width: cell.width.as_f32() as u16,
                cell_height: cell.height.as_f32() as u16,
            }));
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let mode = *self.term.lock().mode();
        if let Some(bytes) = key_bytes(&event.keystroke, mode) {
            cx.stop_propagation();
            self.input(bytes, cx);
        }
    }
    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self
            .term
            .lock()
            .selection_to_string()
            .filter(|t| !t.is_empty())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
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
        let mut term = self.term.lock();
        let mut selection = Selection::new(
            SelectionType::Simple,
            GridPoint::new(term.topmost_line(), Column(0)),
            Side::Left,
        );
        selection.update(
            GridPoint::new(term.bottommost_line(), term.last_column()),
            Side::Right,
        );
        term.selection = Some(selection);
        drop(term);
        cx.notify();
    }

    /// The cell under a window position, and which half of it.
    fn grid_point(&self, position: gpui::Point<gpui::Pixels>) -> Option<(GridPoint, Side)> {
        let g = self.geometry.get()?;
        let x = ((position.x - g.origin.x) / g.cell.width).max(0.);
        let y = ((position.y - g.origin.y) / g.cell.height).max(0.);
        let column = (x as usize).min(self.size.columns.saturating_sub(1));
        let line = (y as usize).min(self.size.lines.saturating_sub(1));
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        let offset = self.term.lock().grid().display_offset();
        Some((
            viewport_to_point(offset, GridPoint::new(line, Column(column))),
            side,
        ))
    }
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((at, side)) = self.grid_point(event.position) else {
            return;
        };
        let mut term = self.term.lock();
        match term.selection.as_mut() {
            Some(selection) if event.modifiers.shift => selection.update(at, side),
            _ => {
                let ty = match event.click_count {
                    2 => SelectionType::Semantic,
                    3.. => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                term.selection = Some(Selection::new(ty, at, side));
            }
        }
        drop(term);
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
        if !self.selecting || !self.visible {
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
            let mut term = self.term.lock();
            if term.selection.is_none() {
                return false;
            }
            let before = term.grid().display_offset();
            term.scroll_display(Scroll::Delta(delta));
            term.grid().display_offset() != before
        };
        if changed {
            if let Some((at, side)) = self.grid_point(position)
                && let Some(selection) = self.term.lock().selection.as_mut()
            {
                selection.update(at, side);
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
        if let Some((at, side)) = self.grid_point(event.position) {
            if let Some(selection) = self.term.lock().selection.as_mut() {
                selection.update(at, side);
            }
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
        let mode = *self.term.lock().mode();
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
            self.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Msg::Shutdown);
        }
        // The hangup a closing shell expects, for it and its job.
        if self.shell_pid > 1 && !self.exited {
            let pid = self.shell_pid as i32;
            unsafe {
                libc::kill(pid, libc::SIGHUP);
                libc::kill(-pid, libc::SIGHUP);
            }
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
            .on_action(cx.listener(|this, _: &SendTab, _, cx| this.input(b"\t".to_vec(), cx)))
            .on_action(
                cx.listener(|this, _: &SendBackTab, _, cx| this.input(b"\x1b[Z".to_vec(), cx)),
            )
            .on_action(cx.listener(|this, _: &SendEscape, _, cx| this.input(b"\x1b".to_vec(), cx)))
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
type Painting = (usize, char, Pen, Option<Hsla>, bool);
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

        let term = term.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset;
        let selection = content.selection;
        let mut rows: Vec<Vec<Painting>> = vec![Vec::new(); lines];
        for indexed in content.display_iter {
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let Some(at) = point_to_viewport(offset, indexed.point) else {
                continue;
            };
            if at.line >= lines {
                continue;
            }
            let mut fg = color(cell.fg, colors, true);
            let mut bg = match cell.bg {
                AnsiColor::Named(NamedColor::Background) => None,
                other => Some(color(other, colors, false)),
            };
            if cell.flags.contains(Flags::INVERSE) {
                let back = bg.unwrap_or(rgb(CHAT).into());
                bg = Some(fg);
                fg = back;
            }
            if cell.flags.intersects(Flags::DIM) {
                fg.a *= 0.66;
            }
            if selection.is_some_and(|s| s.contains(indexed.point)) {
                bg = Some(selection_fill());
            }
            let c = if cell.flags.contains(Flags::HIDDEN) {
                ' '
            } else {
                cell.c
            };
            rows[at.line].push((
                at.column.0,
                c,
                Pen {
                    fg,
                    bold: cell.flags.contains(Flags::BOLD),
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                    strike: cell.flags.contains(Flags::STRIKEOUT),
                },
                bg,
                cell.flags.contains(Flags::WIDE_CHAR),
            ));
        }
        let cursor_point = content.cursor.point;
        let cursor_shape = content.cursor.shape;
        let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR);
        drop(term);

        let mut painted = Painted {
            lines: Vec::new(),
            rects: Vec::new(),
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
                    text.push(cells[end].1);
                    end += 1;
                }
                if end == ix {
                    text.push(cells[ix].1);
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
        if show_cursor
            && cursor_shape != CursorShape::Hidden
            && let Some(at) = point_to_viewport(offset, cursor_point)
            && at.line < lines
        {
            let x = origin.x + cell_width * at.column.0 as f32;
            let y = origin.y + line_height * at.line as f32;
            let accent: Hsla = accent_color();
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

fn fixed_font() -> Font {
    gpui::font(crate::markdown::FIXED_FONT)
}
fn selection_fill() -> Hsla {
    content(0.18)
}
fn write_note(term: &Arc<FairMutex<Term<Listener>>>, text: &str) {
    let mut parser = alacritty_terminal::vte::ansi::Processor::<
        alacritty_terminal::vte::ansi::StdSyncHandler,
    >::new();
    parser.advance(&mut *term.lock(), text.as_bytes());
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
fn hex(rgb: Rgb) -> u32 {
    ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
}
fn to_rgb(value: u32) -> Rgb {
    Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}
/// Reply colour for an OSC 4/10/11/12 query: an app-set colour wins.
fn color_rgb(index: usize, colors: &Colors) -> Rgb {
    if let Some(rgb) = colors[index] {
        return rgb;
    }
    to_rgb(match index {
        0..=255 => indexed_rgb(index as u8),
        256 => OSC_FOREGROUND,
        257 => OSC_BACKGROUND,
        258 => ACCENT,
        _ => OSC_FOREGROUND,
    })
}
fn color(value: AnsiColor, colors: &Colors, foreground: bool) -> Hsla {
    let dim = |c: u32| {
        let mut h: Hsla = rgb(c).into();
        h.a *= 0.66;
        h
    };
    match value {
        AnsiColor::Spec(spec) => rgb(hex(spec)).into(),
        AnsiColor::Indexed(index) => colors[index as usize]
            .map_or_else(|| rgb(indexed_rgb(index)).into(), |c| rgb(hex(c)).into()),
        AnsiColor::Named(name) => {
            if let Some(c) = colors[name] {
                return rgb(hex(c)).into();
            }
            match name {
                NamedColor::Foreground | NamedColor::BrightForeground => content(1.),
                NamedColor::DimForeground => content(0.66),
                NamedColor::Background => {
                    if foreground {
                        rgb(CHAT).into()
                    } else {
                        gpui::transparent_black()
                    }
                }
                NamedColor::Cursor => accent_color(),
                NamedColor::DimBlack => dim(ANSI[0]),
                NamedColor::DimRed => dim(ANSI[1]),
                NamedColor::DimGreen => dim(ANSI[2]),
                NamedColor::DimYellow => dim(ANSI[3]),
                NamedColor::DimBlue => dim(ANSI[4]),
                NamedColor::DimMagenta => dim(ANSI[5]),
                NamedColor::DimCyan => dim(ANSI[6]),
                NamedColor::DimWhite => dim(ANSI[7]),
                other => rgb(ANSI[(other as usize).min(15)]).into(),
            }
        }
    }
}

/// Bytes a keystroke sends, or None to leave it to the app's bindings.
/// Includes terminalKeys.ts's macOS editing shortcuts.
pub(crate) fn key_bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    let key = keystroke.key.as_str();
    if m.platform {
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
        match key {
            "left" => return Some(b"\x1bb".to_vec()),
            "right" => return Some(b"\x1bf".to_vec()),
            _ => {}
        }
    }
    let modifier = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let cursor = |final_byte: char| {
        Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        })
    };
    let tilde = |code: u8| {
        Some(if modifier > 1 {
            format!("\x1b[{code};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        })
    };
    let function = |final_byte: char| {
        Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        } else {
            format!("\x1bO{final_byte}").into_bytes()
        })
    };
    let meta = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if m.alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        Some(out)
    };
    match key {
        "up" => return cursor('A'),
        "down" => return cursor('B'),
        "right" => return cursor('C'),
        "left" => return cursor('D'),
        "home" => return cursor('H'),
        "end" => return cursor('F'),
        "insert" => return tilde(2),
        "delete" => return tilde(3),
        "pageup" => return tilde(5),
        "pagedown" => return tilde(6),
        "f1" => return function('P'),
        "f2" => return function('Q'),
        "f3" => return function('R'),
        "f4" => return function('S'),
        "f5" => return tilde(15),
        "f6" => return tilde(17),
        "f7" => return tilde(18),
        "f8" => return tilde(19),
        "f9" => return tilde(20),
        "f10" => return tilde(21),
        "f11" => return tilde(23),
        "f12" => return tilde(24),
        "enter" => return meta(b"\r"),
        "escape" => return meta(b"\x1b"),
        "tab" if m.shift => return Some(b"\x1b[Z".to_vec()),
        "tab" => return meta(b"\t"),
        "backspace" if m.control => return meta(b"\x08"),
        "backspace" => return meta(b"\x7f"),
        _ => {}
    }
    if m.control {
        let ch = if key == "space" {
            ' '
        } else {
            let mut chars = key.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            ch
        };
        let code = match ch.to_ascii_lowercase() {
            c @ 'a'..='z' => c as u8 - b'a' + 1,
            ' ' | '@' | '2' => 0,
            '[' | '3' => 27,
            '\\' | '4' => 28,
            ']' | '5' => 29,
            '^' | '6' => 30,
            '_' | '-' | '/' | '7' => 31,
            '8' => 127,
            _ => return None,
        };
        return meta(&[code]);
    }
    // Option is Meta (`macOptionIsMeta`): the key itself, after ESC.
    if m.alt {
        let text = if key == "space" {
            " ".to_owned()
        } else if m.shift {
            key.to_uppercase()
        } else {
            key.to_owned()
        };
        if text.chars().count() == 1 {
            return meta(text.as_bytes());
        }
    }
    let text = keystroke.key_char.as_deref().or(match key {
        "space" => Some(" "),
        _ => None,
    })?;
    Some(text.as_bytes().to_vec())
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
