//! A thin main-thread bridge to the native AppKit menu. No original payloads
//! cross this boundary, and closing it releases all presentation copies.
use super::*;
use std::ffi::{CStr, CString, c_char, c_void};

pub(super) enum Event {
    Copy(u64),
    Clear,
    Settings,
    Composer,
    Quit,
    Search(String),
    Closed,
}
pub(super) struct Menu {
    handle: *mut c_void,
    // Swift borrows this stable allocation until destroy returns.
    sender: Box<async_channel::Sender<Event>>,
}
unsafe extern "C" {
    fn vorkspace_clipboard_create(
        callback: extern "C" fn(*mut c_void, u32, u64, *const c_char),
        context: *mut c_void,
    ) -> *mut c_void;
    fn vorkspace_clipboard_destroy(handle: *mut c_void);
    fn vorkspace_clipboard_visible(handle: *mut c_void) -> bool;
    fn vorkspace_clipboard_toggle(handle: *mut c_void, x: f64, y: f64);
    fn vorkspace_clipboard_hide(handle: *mut c_void);
    fn vorkspace_clipboard_begin(handle: *mut c_void);
    fn vorkspace_clipboard_add(
        handle: *mut c_void,
        id: u64,
        title: *const c_char,
        image: bool,
        pixels: *const u8,
        width: usize,
        height: usize,
    );
    fn vorkspace_clipboard_end(handle: *mut c_void, message: *const c_char, enabled: bool);
}
extern "C" fn receive(context: *mut c_void, action: u32, id: u64, text: *const c_char) {
    // Called synchronously on the main thread while Menu owns the sender.
    let sender = unsafe { &*context.cast::<async_channel::Sender<Event>>() };
    let event = match action {
        1 => Event::Copy(id),
        2 => Event::Clear,
        3 => Event::Settings,
        4 => Event::Composer,
        5 => Event::Quit,
        6 => Event::Search(
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned(),
        ),
        _ => Event::Closed,
    };
    // Callbacks enqueue control events, never clipboard payloads or native UI work.
    let _ = sender.try_send(event);
}
impl Menu {
    pub(super) fn new() -> (Self, async_channel::Receiver<Event>) {
        let (tx, rx) = async_channel::unbounded();
        let mut sender = Box::new(tx);
        let handle = unsafe {
            vorkspace_clipboard_create(
                receive,
                (&mut *sender as *mut async_channel::Sender<Event>).cast(),
            )
        };
        (Self { handle, sender }, rx)
    }
    pub(super) fn visible(&self) -> bool {
        unsafe { vorkspace_clipboard_visible(self.handle) }
    }
    pub(super) fn toggle(&self, x: f64, y: f64) {
        unsafe { vorkspace_clipboard_toggle(self.handle, x, y) }
    }
    pub(super) fn hide(&self) {
        unsafe { vorkspace_clipboard_hide(self.handle) }
    }
    pub(super) fn sync(&self, service: &Service) {
        unsafe { vorkspace_clipboard_begin(self.handle) };
        for id in service.history.matching(&service.query) {
            let item = service.history.get(id).unwrap();
            let title = CString::new(item.content.label()).unwrap();
            let (pixels, width, height) = item
                .preview
                .as_ref()
                .map(|p| (p.pixels.as_ptr(), p.width, p.height))
                .unwrap_or((std::ptr::null(), 0, 0));
            unsafe {
                vorkspace_clipboard_add(
                    self.handle,
                    id,
                    title.as_ptr(),
                    matches!(&*item.content, Content::Image { .. }),
                    pixels,
                    width,
                    height,
                )
            };
        }
        let message = CString::new(if !service.enabled {
            "Clipboard history is off"
        } else if service.history.items.is_empty() {
            "Copy text or an image to get started"
        } else {
            "No matching text"
        })
        .unwrap();
        unsafe { vorkspace_clipboard_end(self.handle, message.as_ptr(), service.enabled) };
    }
}
impl Drop for Menu {
    fn drop(&mut self) {
        unsafe { vorkspace_clipboard_destroy(self.handle) };
        self.sender.close();
    }
}
