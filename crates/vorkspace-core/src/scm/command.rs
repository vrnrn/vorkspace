//! Git process setup and bounded output collection.
use crate::{process, shell_env as shell};
use std::{
    io::Read,
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// Any git can start a filter (git-lfs), which a Finder launch's bare PATH
/// lacks. The cached search path finds it without waiting on the login shell.
pub(crate) fn git_cmd() -> Command {
    let mut cmd = Command::new("git");
    process::configure_child(&mut cmd);
    cmd.env("PATH", shell::cached_gui_search_path())
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

pub(super) fn git_cmd_for_args_with_path(
    args: &[&str],
    gui_path: impl FnOnce() -> String,
) -> Command {
    let mut cmd = git_cmd();
    let args = args.strip_prefix(&["--literal-pathspecs"]).unwrap_or(args);
    if matches!(
        args.first().copied(),
        Some("commit" | "push" | "pull" | "fetch" | "clone")
    ) {
        // Signers, hooks, and credential helpers may only be on the login-shell PATH.
        cmd.env("PATH", gui_path());
    }
    cmd
}

fn git_cmd_for_args(args: &[&str]) -> Command {
    git_cmd_for_args_with_path(args, shell::gui_search_path)
}

pub(super) fn git_checked(root: &Path, args: &[&str]) -> Result<(), String> {
    let mut cmd = git_cmd_for_args(args);
    let output = cmd
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let msg = stderr.trim();
    if !msg.is_empty() {
        return Err(msg.to_string());
    }
    let msg = stdout.trim();
    if !msg.is_empty() {
        return Err(msg.to_string());
    }
    Err(format!("git {} failed", args.join(" ")))
}

pub(super) fn git_run(root: &Path, args: &[&str]) -> Option<String> {
    git_output(root, args).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

pub(super) fn git_output(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = git_cmd_for_args(args)
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    if git_status_ok(&output.status, args) {
        return Some(output.stdout);
    }
    None
}

fn git_status_ok(status: &std::process::ExitStatus, args: &[&str]) -> bool {
    status.success()
        || (status.code() == Some(1) && matches!(args.first().copied(), Some("diff" | "grep")))
}

pub(super) fn git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let output = git_cmd().arg("-C").arg(root).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub(crate) fn git_output_capped(
    root: &Path,
    args: &[&str],
    max_bytes: usize,
    cancel: Option<&AtomicBool>,
) -> Option<(Vec<u8>, bool)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let expired =
        || Instant::now() >= deadline || cancel.is_some_and(|token| token.load(Ordering::Acquire));
    if expired() {
        return None;
    }
    let mut child = git_cmd()
        .process_group(0)
        .arg("--no-pager")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(mut stdout) = child.stdout.take() else {
        process::terminate(&mut child);
        return None;
    };
    let (sender, receiver) = mpsc::sync_channel(8);
    let reader = thread::Builder::new()
        .name("git-output-reader".to_string())
        .spawn(move || {
            let mut chunk = vec![0u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => {
                        let _ = sender.send(Ok(Vec::new()));
                        break;
                    }
                    Ok(read) => {
                        if sender.send(Ok(chunk[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });
    let reader = match reader {
        Ok(reader) => reader,
        Err(_) => {
            process::terminate(&mut child);
            return None;
        }
    };

    let mut buf = Vec::new();
    loop {
        if expired() {
            process::terminate(&mut child);
            return None;
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(chunk)) if chunk.is_empty() => break,
            Ok(Ok(chunk)) => {
                let remaining = max_bytes.saturating_sub(buf.len());
                if chunk.len() > remaining {
                    buf.extend_from_slice(&chunk[..remaining]);
                    process::terminate(&mut child);
                    return Some((buf, true));
                }
                buf.extend_from_slice(&chunk);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                process::terminate(&mut child);
                return None;
            }
        }
    }
    // Joining is safe once the reader has sent EOF. A process can close its
    // output before exiting, so the same deadline also bounds the reap.
    let _ = reader.join();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if !expired() => thread::sleep(Duration::from_millis(1)),
            _ => {
                process::terminate(&mut child);
                return None;
            }
        }
    };
    if git_status_ok(&status, args) {
        Some((buf, false))
    } else {
        None
    }
}
