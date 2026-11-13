//! Safe, exclusively owned access to the pinned public libghostty-vt C API.
//! The caller serializes a terminal's mutation, selection and render capture.
pub use smol_str::SmolStr as Text;
use std::{ffi::c_void, io, ops::BitOr, ptr::NonNull};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes(u32);
impl Modes {
    pub const APP_CURSOR: Self = Self(1);
    pub const BRACKETED_PASTE: Self = Self(2);
    pub const ALTERNATE_SCROLL: Self = Self(4);
    pub const ALT_SCREEN: Self = Self(8);
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}
impl BitOr for Modes {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

pub struct Config {
    pub scrollback_lines: usize,
    pub scrollback_bytes: usize,
    pub palette: [u32; 256],
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    PtyWrite(Vec<u8>),
    ClipboardStore(String),
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub column: usize,
    pub text: Text,
    pub width: usize,
    pub foreground: u32,
    pub background: u32,
    pub foreground_set: bool,
    pub background_set: bool,
    pub selected: bool,
    pub flags: u32,
}
impl Cell {
    pub const BOLD: u32 = 1;
    pub const ITALIC: u32 = 2;
    pub const DIM: u32 = 4;
    pub const INVERSE: u32 = 16;
    pub const HIDDEN: u32 = 32;
    pub const STRIKE: u32 = 64;
    pub const UNDERLINE: u32 = 0x700;
    pub fn has(&self, flags: u32) -> bool {
        self.flags & flags != 0
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Beam,
    Block,
    Underline,
    Hollow,
}
#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub column: usize,
    pub row: usize,
    pub shape: CursorShape,
    pub color: u32,
}
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub rows: Vec<Vec<Cell>>,
    pub columns: usize,
    pub cursor: Option<Cursor>,
    pub foreground: u32,
    pub background: u32,
}
impl Frame {
    pub fn text(&self) -> String {
        self.rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| if c.text.is_empty() { " " } else { &c.text })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
#[derive(Clone, Copy)]
pub enum SelectionKind {
    Simple,
    Word,
    Line,
}

#[derive(Clone, Copy)]
pub enum KeyAction {
    Release,
    Press,
    Repeat,
}
pub struct Key<'a> {
    pub name: &'a str,
    pub text: &'a str,
    pub unshifted_codepoint: u32,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub action: KeyAction,
}

#[repr(C)]
struct RawCell {
    row: u32,
    column: u32,
    width: u32,
    foreground: u32,
    background: u32,
    flags: u32,
    foreground_set: u32,
    background_set: u32,
    selected: u32,
}
#[repr(C)]
#[derive(Default)]
struct RawFrame {
    columns: u32,
    rows: u32,
    cursor_x: u32,
    cursor_y: u32,
    cursor_shape: u32,
    cursor_visible: u32,
    foreground: u32,
    background: u32,
    cursor: u32,
}
type EffectCallback = unsafe extern "C" fn(*mut c_void, u32, *const u8, usize);
type CellCallback = unsafe extern "C" fn(*mut c_void, *const RawCell, *const u8, usize);
unsafe extern "C" {
    fn vorkspace_vt_new(
        cols: u16,
        rows: u16,
        lines: usize,
        bytes: usize,
        palette: *const u32,
        fg: u32,
        bg: u32,
        cursor: u32,
        effect: EffectCallback,
        userdata: *mut c_void,
    ) -> *mut c_void;
    fn vorkspace_vt_free(vt: *mut c_void);
    fn vorkspace_vt_write(vt: *mut c_void, bytes: *const u8, len: usize);
    fn vorkspace_vt_resize(vt: *mut c_void, cols: u16, rows: u16, cw: u32, ch: u32) -> i32;
    fn vorkspace_vt_frame(
        vt: *mut c_void,
        frame: *mut RawFrame,
        paint: CellCallback,
        userdata: *mut c_void,
    ) -> i32;
    fn vorkspace_vt_modes(vt: *mut c_void) -> u32;
    fn vorkspace_vt_display_offset(vt: *mut c_void) -> u64;
    fn vorkspace_vt_scroll(vt: *mut c_void, delta: i32, bottom: u32);
    fn vorkspace_vt_select(
        vt: *mut c_void,
        x: u16,
        y: u16,
        right: u32,
        kind: u32,
        extend: u32,
    ) -> i32;
    fn vorkspace_vt_select_all(vt: *mut c_void) -> i32;
    fn vorkspace_vt_clear_selection(vt: *mut c_void);
    fn vorkspace_vt_selection_text(
        vt: *mut c_void,
        buf: *mut u8,
        cap: usize,
        len: *mut usize,
    ) -> i32;
    fn vorkspace_vt_expire_hold(vt: *mut c_void) -> i32;
    fn vorkspace_vt_hold_remaining(vt: *mut c_void) -> u32;
    fn vorkspace_vt_default_colors(vt: *mut c_void, fg: u32, bg: u32, cursor: u32) -> i32;
    fn vorkspace_vt_key(
        vt: *mut c_void,
        name: *const u8,
        name_len: usize,
        mods: u32,
        codepoint: u32,
        action: u32,
        text: *const u8,
        text_len: usize,
        out: *mut u8,
        capacity: usize,
        written: *mut usize,
    ) -> i32;
}

/// Never shared without exclusive access. No callback re-enters this object;
/// callback data is copied into its stable boxed queue before returning to C.
struct Callbacks {
    effects: Vec<Effect>,
}
pub struct Terminal {
    raw: NonNull<c_void>,
    callbacks: Box<Callbacks>,
}
// SAFETY: libghostty-vt has no thread affinity. &mut self gates every call,
// including reads that update internal caches. No references escape C.
unsafe impl Send for Terminal {}

fn checked(result: i32) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!("libghostty-vt error {result}")))
    }
}
unsafe fn borrowed_bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}
unsafe extern "C" fn effect(userdata: *mut c_void, kind: u32, ptr: *const u8, len: usize) {
    let effects = &mut unsafe { &mut *userdata.cast::<Callbacks>() }.effects;
    let bytes = unsafe { borrowed_bytes(ptr, len) };
    effects.push(if kind == 0 {
        Effect::PtyWrite(bytes.to_vec())
    } else {
        Effect::ClipboardStore(String::from_utf8_lossy(bytes).into_owned())
    });
}
unsafe extern "C" fn paint_cell(
    userdata: *mut c_void,
    raw: *const RawCell,
    ptr: *const u8,
    len: usize,
) {
    let rows = unsafe { &mut *userdata.cast::<Vec<Vec<Cell>>>() };
    let raw = unsafe { &*raw };
    // libghostty emits rows in order; handle even empty terminal rows.
    rows.resize_with((raw.row as usize + 1).max(rows.len()), Vec::new);
    rows[raw.row as usize].push(Cell {
        column: raw.column as usize,
        text: Text::new(String::from_utf8_lossy(unsafe { borrowed_bytes(ptr, len) })),
        width: raw.width as usize,
        foreground: raw.foreground,
        background: raw.background,
        foreground_set: raw.foreground_set != 0,
        background_set: raw.background_set != 0,
        selected: raw.selected != 0,
        flags: raw.flags,
    });
}
impl Terminal {
    pub fn new(cols: u16, rows: u16, config: &Config) -> io::Result<Self> {
        if cols == 0 || rows == 0 {
            return Err(io::Error::other("terminal dimensions must be positive"));
        }
        let mut callbacks = Box::new(Callbacks {
            effects: Vec::new(),
        });
        let raw = unsafe {
            vorkspace_vt_new(
                cols,
                rows,
                config.scrollback_lines,
                config.scrollback_bytes,
                config.palette.as_ptr(),
                config.foreground,
                config.background,
                config.cursor,
                effect,
                (&mut *callbacks as *mut Callbacks).cast(),
            )
        };
        Ok(Self {
            raw: NonNull::new(raw)
                .ok_or_else(|| io::Error::other("could not allocate libghostty-vt terminal"))?,
            callbacks,
        })
    }
    pub fn write(&mut self, bytes: &[u8]) {
        unsafe { vorkspace_vt_write(self.raw.as_ptr(), bytes.as_ptr(), bytes.len()) }
    }
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.callbacks.effects)
    }
    pub fn resize(&mut self, cols: u16, rows: u16, cw: u32, ch: u32) -> io::Result<()> {
        checked(unsafe { vorkspace_vt_resize(self.raw.as_ptr(), cols, rows, cw, ch) })
    }
    pub fn frame(&mut self) -> io::Result<Frame> {
        let mut raw = RawFrame::default();
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        checked(unsafe {
            vorkspace_vt_frame(
                self.raw.as_ptr(),
                &mut raw,
                paint_cell,
                (&mut rows as *mut Vec<Vec<Cell>>).cast(),
            )
        })?;
        rows.resize_with(raw.rows as usize, Vec::new);
        Ok(Frame {
            rows,
            columns: raw.columns as usize,
            foreground: raw.foreground,
            background: raw.background,
            cursor: (raw.cursor_visible != 0).then_some(Cursor {
                column: raw.cursor_x as usize,
                row: raw.cursor_y as usize,
                shape: match raw.cursor_shape {
                    1 => CursorShape::Block,
                    2 => CursorShape::Underline,
                    3 => CursorShape::Hollow,
                    _ => CursorShape::Beam,
                },
                color: raw.cursor,
            }),
        })
    }
    pub fn modes(&mut self) -> Modes {
        Modes(unsafe { vorkspace_vt_modes(self.raw.as_ptr()) })
    }
    pub fn display_offset(&mut self) -> u64 {
        unsafe { vorkspace_vt_display_offset(self.raw.as_ptr()) }
    }
    pub fn scroll(&mut self, lines: i32) {
        unsafe { vorkspace_vt_scroll(self.raw.as_ptr(), lines, 0) }
    }
    pub fn scroll_to_bottom(&mut self) {
        unsafe { vorkspace_vt_scroll(self.raw.as_ptr(), 0, 1) }
    }
    pub fn clear_selection(&mut self) {
        unsafe { vorkspace_vt_clear_selection(self.raw.as_ptr()) }
    }
    pub fn select(
        &mut self,
        x: u16,
        y: u16,
        right: bool,
        kind: SelectionKind,
        extend: bool,
    ) -> io::Result<()> {
        checked(unsafe {
            vorkspace_vt_select(
                self.raw.as_ptr(),
                x,
                y,
                right as u32,
                match kind {
                    SelectionKind::Simple => 0,
                    SelectionKind::Word => 1,
                    SelectionKind::Line => 2,
                },
                extend as u32,
            )
        })
    }
    pub fn select_all(&mut self) -> io::Result<()> {
        checked(unsafe { vorkspace_vt_select_all(self.raw.as_ptr()) })
    }
    pub fn selection_text(&mut self) -> Option<String> {
        let mut len = 0;
        unsafe {
            vorkspace_vt_selection_text(self.raw.as_ptr(), std::ptr::null_mut(), 0, &mut len);
        }
        if len == 0 {
            return None;
        }
        let mut bytes = vec![0; len];
        checked(unsafe {
            vorkspace_vt_selection_text(
                self.raw.as_ptr(),
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut len,
            )
        })
        .ok()?;
        bytes.truncate(len);
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
    pub fn key(&mut self, key: Key<'_>) -> io::Result<Vec<u8>> {
        if char::from_u32(key.unshifted_codepoint).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid terminal key codepoint",
            ));
        }
        let mut bytes = vec![0; key.text.len().saturating_mul(16).max(128)];
        let mut len = 0;
        let mods = key.shift as u32 | ((key.control as u32) << 1) | ((key.alt as u32) << 2);
        checked(unsafe {
            vorkspace_vt_key(
                self.raw.as_ptr(),
                key.name.as_ptr(),
                key.name.len(),
                mods,
                key.unshifted_codepoint,
                match key.action {
                    KeyAction::Release => 0,
                    KeyAction::Press => 1,
                    KeyAction::Repeat => 2,
                },
                key.text.as_ptr(),
                key.text.len(),
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut len,
            )
        })?;
        bytes.truncate(len);
        Ok(bytes)
    }
    pub fn render_hold_remaining(&mut self) -> Option<std::time::Duration> {
        let millis = unsafe { vorkspace_vt_hold_remaining(self.raw.as_ptr()) };
        (millis != u32::MAX).then(|| std::time::Duration::from_millis(millis as u64))
    }
    pub fn set_default_colors(&mut self, fg: u32, bg: u32, cursor: u32) -> io::Result<()> {
        checked(unsafe { vorkspace_vt_default_colors(self.raw.as_ptr(), fg, bg, cursor) })
    }
    pub fn expire_render_hold(&mut self) -> bool {
        unsafe { vorkspace_vt_expire_hold(self.raw.as_ptr()) != 0 }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { vorkspace_vt_free(self.raw.as_ptr()) }
    }
}

#[cfg(test)]
mod tests;
