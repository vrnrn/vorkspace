//! Some unrelated programs also install `pi` or `omp`. As in the
//! legacy binary resolver, verify their CLI contract before marking them installed.
use super::*;
use crate::process;
use std::sync::OnceLock;
use std::{
    path::Path,
    process::{Command, Stdio},
    sync::{mpsc, Mutex},
    thread,
};
type IdentityCache = HashMap<(Kind, PathBuf), Option<std::time::SystemTime>>;
pub(super) fn matches(kind: Kind, path: &Path) -> bool {
    if !kind.is_supported() {
        return false;
    }
    if !matches!(kind, Kind::Pi | Kind::Omp) {
        return true;
    }
    static CACHE: OnceLock<Mutex<IdentityCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (kind, path.to_path_buf());
    let modified = path.metadata().ok().and_then(|m| m.modified().ok());
    if cache.lock().unwrap().get(&key) == Some(&modified) {
        return true;
    }
    let mut command = Command::new(path);
    command
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::shell_env::apply_gui_env(&mut command);
    process::isolate(&mut command);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let _registration = process::register(child.id());
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.take(64 * 1024).read_to_string(&mut text);
        let _ = tx.send(text);
    });
    let text = rx.recv_timeout(Duration::from_secs(3)).unwrap_or_default();
    process::terminate(&mut child);
    drop(rx);
    let _ = reader.join();
    let valid = help_matches(kind, &text.to_lowercase());
    if valid {
        cache.lock().unwrap().insert(key, modified);
    }
    valid
}
fn help_matches(kind: Kind, text: &str) -> bool {
    match kind {
        Kind::Pi | Kind::Omp => text.contains("--mode") && text.contains("rpc"),
        _ => true,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unrelated_binaries_with_the_same_name() {
        assert!(!help_matches(
            Kind::Pi,
            "pi: print digits of pi --precision"
        ));
        assert!(help_matches(Kind::Pi, "pi --mode rpc"));
    }
}
