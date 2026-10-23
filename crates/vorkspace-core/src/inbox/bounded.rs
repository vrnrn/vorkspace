//! Integration responses are bounded before allocation/parsing, including CLI
//! stderr. A failed/oversized lookup never becomes partial ticket context.
use std::{
    io::Read,
    process::{Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
const MAX_BYTES: usize = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
pub(super) fn body(reader: impl Read) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Integration returned an unreadable response".to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("Integration response exceeds the 4 MiB limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "Integration returned invalid UTF-8".into())
}
enum Chunk {
    Data(bool, Vec<u8>),
    End,
    Error(String),
}
fn drain(mut reader: impl Read, stderr: bool, tx: mpsc::SyncSender<Chunk>) {
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                let _ = tx.send(Chunk::End);
                return;
            }
            Ok(n) => {
                if tx.send(Chunk::Data(stderr, buffer[..n].to_vec())).is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = tx.send(Chunk::Error(e.to_string()));
                return;
            }
        }
    }
}
pub(super) fn output(command: &mut Command) -> Result<Output, String> {
    crate::process::isolate(command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::sync_channel(4);
    let stdout = child
        .stdout
        .take()
        .ok_or("GitHub CLI stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("GitHub CLI stderr is unavailable")?;
    std::thread::spawn({
        let tx = tx.clone();
        move || drain(stdout, false, tx)
    });
    std::thread::spawn(move || drain(stderr, true, tx));
    let started = Instant::now();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut ended = 0;
    let result = loop {
        if started.elapsed() >= TIMEOUT {
            break Err("GitHub CLI lookup timed out after 20 seconds".into());
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(Chunk::Data(stderr, bytes)) => {
                if out.len() + err.len() + bytes.len() > MAX_BYTES {
                    break Err("GitHub CLI output exceeds the 4 MiB limit".into());
                }
                if stderr {
                    err.extend(bytes);
                } else {
                    out.extend(bytes);
                }
            }
            Ok(Chunk::End) => ended += 1,
            Ok(Chunk::Error(error)) => break Err(error),
            Err(mpsc::RecvTimeoutError::Disconnected) if ended < 2 => {
                break Err("GitHub CLI output closed unexpectedly".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if ended == 2 {
            match child.try_wait() {
                Ok(Some(status)) => {
                    break Ok(Output {
                        status,
                        stdout: out,
                        stderr: err,
                    })
                }
                Ok(None) => {}
                Err(error) => break Err(error.to_string()),
            }
        }
    };
    if result.is_err() {
        crate::process::terminate(&mut child);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_limits_apply_before_parsing_and_reject_partial_utf8() {
        assert_eq!(body("🙂".as_bytes()).unwrap(), "🙂");
        assert!(body(std::io::repeat(b'x').take(MAX_BYTES as u64 + 100))
            .unwrap_err()
            .contains("4 MiB"));
        assert!(body(&[0xff][..]).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn cli_bounds_stdout_and_stderr_and_reaps_noisy_children() {
        for redirect in ["", " >&2"] {
            let mut command = Command::new("/bin/sh");
            command.args([
                "-c",
                &format!("while :; do printf '%020000d'{redirect} 0; done"),
            ]);
            assert!(output(&mut command).unwrap_err().contains("4 MiB"));
        }
    }
}
