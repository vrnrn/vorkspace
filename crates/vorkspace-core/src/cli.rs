//! Bounded CLI invocations for Settings management. Call only from workers.
mod discovery;
use crate::kind::Kind;
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{OnceLock, RwLock},
    time::{Duration, Instant},
};
fn paths() -> &'static RwLock<HashMap<String, String>> {
    static PATHS: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    PATHS.get_or_init(Default::default)
}
pub fn initialize(overrides: &HashMap<String, String>) {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        *paths().write().unwrap() = overrides.clone();
    });
}
pub fn configured(kind: Kind) -> Option<PathBuf> {
    paths()
        .read()
        .unwrap()
        .get(kind.id())
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
}
pub fn output(
    binary: &Path,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> Result<String, String> {
    let mut command = Command::new(binary);
    crate::shell_env::apply_gui_env(&mut command);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::isolate(&mut command);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let _registration = crate::process::register(child.id());
    let read = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut result = Vec::new();
            let mut chunk = [0; 4096];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                result.extend_from_slice(&chunk[..n]);
                if result.len() > 65536 {
                    result.drain(..result.len() - 65536);
                }
            }
            String::from_utf8_lossy(&result).trim().to_owned()
        })
    };
    let stdout = read(Box::new(child.stdout.take().unwrap()));
    let stderr = read(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + timeout;
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Err(e) => break Err(e.to_string()),
            Ok(None) if Instant::now() >= deadline => break Err("CLI command timed out".into()),
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    crate::process::terminate(&mut child);
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    if result?.success() {
        Ok(out)
    } else {
        Err(format!("{err} {out}").trim().to_owned())
    }
}
#[derive(Clone, Debug)]
pub struct Inspection {
    pub path: PathBuf,
    pub version: String,
}
pub fn inspect(kind: Kind, path: Option<&str>) -> Result<Inspection, String> {
    if !kind.is_supported() {
        return Err("This provider is no longer supported.".into());
    }
    let path = match path.filter(|s| !s.trim().is_empty()) {
        Some(p) => {
            let p = PathBuf::from(p);
            if !p.is_absolute() {
                return Err("Enter an absolute path to the CLI executable.".into());
            }
            p
        }
        None => resolve_auto(kind).ok_or("CLI could not be resolved")?,
    };
    if !path.is_file() {
        return Err("CLI executable does not exist.".into());
    }
    let cwd = crate::shell_env::dirs_home()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let version = output(&path, &["--version".into()], &cwd, Duration::from_secs(10))?;
    if kind == Kind::Codex
        && !regex::Regex::new(r"^codex-cli\s+\d+\.\d+\.\d+")
            .unwrap()
            .is_match(&version)
    {
        return Err("Codex CLI returned an invalid version.".into());
    }
    Ok(Inspection { path, version })
}

/// GUI apps may not inherit the login shell PATH. Try the same common user and
/// system install locations as harness.rs, plus Claude's versioned native install.
/// An explicit override wins and errors at launch rather than silently falling back.
pub fn resolve(kind: Kind) -> Option<PathBuf> {
    if !kind.is_supported() {
        return None;
    }
    if let Some(path) = configured(kind) {
        return Some(path);
    }
    resolve_auto(kind)
}
pub fn resolve_auto(kind: Kind) -> Option<PathBuf> {
    if !kind.is_supported() {
        return None;
    }
    let key = format!("VORKSPACE_{}_BINARY", kind.id().to_uppercase());
    if let Some(path) = std::env::var_os(key).filter(|s| !s.is_empty()) {
        return Some(path.into());
    }
    let mut candidates: Vec<PathBuf> =
        Some(std::ffi::OsString::from(crate::shell_env::gui_search_path()))
            .into_iter()
            .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.join(kind.binary_name()))
            .collect();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        for dir in kind.install_dirs().iter().copied().chain([
            ".local/bin",
            ".bun/bin",
            ".npm-global/bin",
            ".cargo/bin",
            "n/bin",
        ]) {
            candidates.push(home.join(dir).join(kind.binary_name()));
        }
        if kind == Kind::Claude {
            candidates.push(home.join(".claude/local/claude"));
            let dir = home.join(".local/share/claude/versions");
            let mut versions: Vec<_> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .collect();
            versions.sort_by_key(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .split('.')
                    .map(|s| s.parse::<u32>().unwrap_or(0))
                    .collect::<Vec<_>>()
            });
            candidates.extend(versions.into_iter().rev());
        }
    }
    for dir in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/snap/bin",
    ] {
        candidates.push(Path::new(dir).join(kind.binary_name()));
    }
    if kind == Kind::Pi {
        let alias = "pi-coding-agent";
        let paths: Vec<_> = candidates
            .iter()
            .filter_map(|p| p.parent().map(|dir| dir.join(alias)))
            .collect();
        candidates.extend(paths);
    }
    candidates
        .into_iter()
        .find_map(|p| (executable(&p) && discovery::matches(kind, &p)).then_some(p))
}
fn executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn validates_executable_and_provider_version_before_saving() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLI with spaces");
        std::fs::write(&path, "#!/bin/sh\nprintf 'codex-cli 1.2.3\\n'\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            inspect(Kind::Codex, path.to_str()).unwrap().version,
            "codex-cli 1.2.3"
        );
        assert!(inspect(Kind::Codex, Some("relative/path"))
            .unwrap_err()
            .contains("absolute"));
        std::fs::write(&path, "#!/bin/sh\nprintf '1.10.3\\n'\n").unwrap();
        assert!(inspect(Kind::Codex, path.to_str())
            .unwrap_err()
            .contains("invalid version"));
    }

    #[test]
    fn unsupported_provider_never_executes_a_configured_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retired-cli");
        let marker = dir.path().join("executed");
        std::fs::write(&path, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(inspect(Kind::Unsupported, path.to_str())
            .unwrap_err()
            .contains("no longer supported"));
        assert!(!marker.exists());
    }
}
