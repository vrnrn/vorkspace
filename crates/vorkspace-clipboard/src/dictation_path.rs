//! Shared user storage, independent of the app bundle or its per-install socket.
use crate::ModelId;
use std::path::{Path, PathBuf};
pub fn resolve(
    home: Option<&Path>,
    system_home: Option<&Path>,
    model: ModelId,
) -> Result<PathBuf, String> {
    home.filter(|p|p.is_absolute()).or_else(||system_home.filter(|p|p.is_absolute()))
        .map(|p|p.join(".vorkspace/dictation").join(model.descriptor().filename))
        .ok_or_else(||"Could not locate your home folder. Restore HOME to your absolute home path, then retry dictation setup.".into())
}
pub fn model_path(model: ModelId) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let system = system_home();
    resolve(home.as_deref(), system.as_deref(), model)
}
fn system_home() -> Option<PathBuf> {
    use std::{
        ffi::{CStr, OsStr},
        os::unix::ffi::OsStrExt,
    };
    let mut size = 4096;
    while size <= 262144 {
        let mut buf = vec![0u8; size];
        let mut pwd = unsafe { std::mem::zeroed::<libc::passwd>() };
        let mut result = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut pwd,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE {
            size *= 2;
            continue;
        }
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        return Some(PathBuf::from(OsStr::from_bytes(
            unsafe { CStr::from_ptr(pwd.pw_dir) }.to_bytes(),
        )));
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    const MODEL_NAME: &str = ModelId::UnifiedEnglish.descriptor().filename;
    fn resolve(home: Option<&Path>, system: Option<&Path>) -> Result<PathBuf, String> {
        super::resolve(home, system, ModelId::default())
    }
    #[test]
    fn shared_user_destination_is_stable_across_bundle_relocations() {
        let home = Path::new("/Users/example");
        let expected = home.join(".vorkspace/dictation").join(MODEL_NAME);
        for _bundle in ["/Applications/vorkspace.app", "/tmp/dev/vorkspace.app"] {
            assert_eq!(resolve(Some(home), None).unwrap(), expected);
        }
    }
    #[test]
    fn missing_or_relative_home_uses_account_home_or_fails_actionably() {
        assert_eq!(
            resolve(None, Some(Path::new("/Users/account"))).unwrap(),
            Path::new("/Users/account/.vorkspace/dictation").join(MODEL_NAME)
        );
        assert_eq!(
            resolve(
                Some(Path::new("relative")),
                Some(Path::new("/Users/account"))
            )
            .unwrap(),
            resolve(None, Some(Path::new("/Users/account"))).unwrap()
        );
        assert!(resolve(None, None).unwrap_err().contains("home folder"));
        assert!(resolve(Some(Path::new("")), None).is_err());
    }
}
