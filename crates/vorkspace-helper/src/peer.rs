//! A packaged helper accepts its own desktop executable, never a plugin process.
use std::{
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStringExt, net::UnixStream},
    },
    path::{Path, PathBuf},
};
fn packaged_app(helper: &Path) -> Option<PathBuf> {
    if !helper.ends_with("Helpers/vorkspace helper.app/Contents/MacOS/vorkspace-helper") {
        return None;
    }
    Some(helper.ancestors().nth(5)?.join("MacOS/vorkspace-gpui"))
}
fn executable(stream: &UnixStream) -> Option<PathBuf> {
    let mut pid: libc::pid_t = 0;
    let mut size = std::mem::size_of_val(&pid) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut size,
        )
    } != 0
        || pid <= 0
    {
        return None;
    }
    let mut path = [0u8; 4096];
    let count = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    if count <= 0 {
        return None;
    }
    let end = path.iter().position(|b| *b == 0)?;
    PathBuf::from(std::ffi::OsString::from_vec(path[..end].to_vec()))
        .canonicalize()
        .ok()
}
pub fn trusted(stream: &UnixStream) -> bool {
    let Ok(helper) = std::env::current_exe() else {
        return false;
    };
    // Direct Cargo binaries keep the existing owner-only development protocol.
    let Some(expected) = packaged_app(&helper) else {
        return true;
    };
    expected
        .canonicalize()
        .ok()
        .is_some_and(|expected| executable(stream).as_ref() == Some(&expected))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packaged_identity_is_exact_and_does_not_confuse_development_binaries() {
        let helper = Path::new(
            "/tmp/vorkspace.app/Contents/Helpers/vorkspace helper.app/Contents/MacOS/vorkspace-helper",
        );
        assert_eq!(
            packaged_app(helper).unwrap(),
            Path::new("/tmp/vorkspace.app/Contents/MacOS/vorkspace-gpui")
        );
        assert!(packaged_app(Path::new("/tmp/native-dev/vorkspace-helper")).is_none());
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(
            executable(&a),
            std::env::current_exe().unwrap().canonicalize().ok()
        );
    }
}
