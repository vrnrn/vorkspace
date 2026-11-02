//! Thread-confined bridge to the pinned FluidAudio Core ML EOU engine.
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    os::unix::ffi::OsStrExt,
    path::Path,
    ptr,
};

unsafe extern "C" {
    fn vorkspace_eou_supported() -> c_int;
    fn vorkspace_eou_load(path: *const c_char, error: *mut *mut c_char) -> *mut c_void;
    fn vorkspace_eou_reset(model: *mut c_void, error: *mut *mut c_char) -> c_int;
    fn vorkspace_eou_feed(
        model: *mut c_void,
        samples: *const f32,
        count: c_int,
        error: *mut *mut c_char,
    ) -> *mut c_char;
    fn vorkspace_eou_finish(model: *mut c_void, error: *mut *mut c_char) -> *mut c_char;
    fn vorkspace_eou_free(model: *mut c_void);
}

pub fn supported() -> bool {
    unsafe { vorkspace_eou_supported() == 1 }
}

pub(crate) struct Engine(*mut c_void);
impl Engine {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "The model path contains a NUL byte".to_owned())?;
        let mut error = ptr::null_mut();
        let model = unsafe { vorkspace_eou_load(path.as_ptr(), &mut error) };
        if model.is_null() {
            return Err(take_error(error));
        }
        Ok(Self(model))
    }

    pub(crate) fn reset(&self) -> Result<(), String> {
        let mut error = ptr::null_mut();
        if unsafe { vorkspace_eou_reset(self.0, &mut error) } != 0 {
            return Err(take_error(error));
        }
        Ok(())
    }

    pub(crate) fn feed(&self, samples: &[f32]) -> Result<String, String> {
        let count = c_int::try_from(samples.len()).map_err(|_| "Audio block is too large")?;
        let mut error = ptr::null_mut();
        let text = unsafe { vorkspace_eou_feed(self.0, samples.as_ptr(), count, &mut error) };
        take_text(text, error)
    }

    pub(crate) fn finish(&self) -> Result<String, String> {
        let mut error = ptr::null_mut();
        let text = unsafe { vorkspace_eou_finish(self.0, &mut error) };
        take_text(text, error)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe { vorkspace_eou_free(self.0) };
    }
}

fn take_text(text: *mut c_char, error: *mut c_char) -> Result<String, String> {
    if text.is_null() {
        return Err(take_error(error));
    }
    let result = unsafe { CStr::from_ptr(text) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| "Parakeet Flash returned invalid text".to_owned());
    unsafe { libc::free(text.cast()) };
    result
}

fn take_error(error: *mut c_char) -> String {
    if error.is_null() {
        return "Parakeet Flash could not complete the request".into();
    }
    let message = unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned();
    unsafe { libc::free(error.cast()) };
    message
}
