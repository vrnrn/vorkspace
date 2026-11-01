//! The window owns desktop blur; the renderer owns its saved color and opacity.
use gpui::{Window, WindowBackgroundAppearance};

pub fn apply(window: &mut Window, glass: bool, radius: u32) {
    if macos::set_blur(window, if glass { radius.min(100) } else { 0 }) {
        // A native material would add another fixed blur over the user's radius.
        window.set_background_appearance(if glass {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        });
        return;
    }
    window.set_background_appearance(if glass {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Opaque
    });
}

mod macos {
    use super::*;
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::OnceLock;

    type Connection = unsafe extern "C" fn() -> i32;
    type SetBlur = unsafe extern "C" fn(i32, i32, i32) -> i32;

    fn functions() -> Option<(Connection, SetBlur)> {
        static FUNCTIONS: OnceLock<Option<(Connection, SetBlur)>> = OnceLock::new();
        *FUNCTIONS.get_or_init(|| unsafe {
            let set = libc::dlsym(
                libc::RTLD_DEFAULT,
                c"CGSSetWindowBackgroundBlurRadius".as_ptr(),
            );
            let mut connection = libc::dlsym(
                libc::RTLD_DEFAULT,
                c"CGSDefaultConnectionForThread".as_ptr(),
            );
            if connection.is_null() {
                connection = libc::dlsym(libc::RTLD_DEFAULT, c"CGSMainConnectionID".as_ptr());
            }
            if set.is_null() || connection.is_null() {
                return None;
            }
            // These optional WindowServer entry points have the same ABI used
            // by the earlier desktop app. Missing support keeps AppKit's blur.
            Some((
                std::mem::transmute::<*mut libc::c_void, Connection>(connection),
                std::mem::transmute::<*mut libc::c_void, SetBlur>(set),
            ))
        })
    }

    pub(super) fn set_blur(window: &Window, radius: u32) -> bool {
        let Some((connection, set)) = functions() else {
            return false;
        };
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return false;
        };
        // GPUI owns the live content view and invokes this on the UI thread.
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        let Some(native) = view.window() else {
            return false;
        };
        let id = native.windowNumber();
        let connection = unsafe { connection() };
        id > 0 && connection != 0 && unsafe { set(connection, id as i32, radius as i32) == 0 }
    }
}
