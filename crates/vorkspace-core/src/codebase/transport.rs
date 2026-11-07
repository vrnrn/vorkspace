//! The app owns the stdio client, not an upstream installer or HTTP server.
use super::engine::check_cancel;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    path::Path,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

const FRAME_LIMIT: usize = 512 * 1024;
pub(super) struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
    errors: ChildStderr,
    pending: Vec<u8>,
    diagnostic: Vec<u8>,
    next_id: u64,
    pub(super) broken: bool,
    _registration: crate::process::Registration,
}

pub(super) fn command(binary: &Path, cache: &Path, runtime: &Path, root: &Path) -> Command {
    let mut command = Command::new(binary);
    // Inherited CBM flags must not change the private namespace, enable a web
    // listener, or permit indexing outside the requested working copy.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CBM_") {
            command.env_remove(name);
        }
    }
    command
        .current_dir(root)
        .env("CBM_CACHE_DIR", cache)
        .env("CBM_RUNTIME_DIR", runtime)
        .env("CBM_ALLOWED_ROOT", root)
        .env("CBM_WORKERS", "2")
        .env("CBM_MEM_BUDGET_MB", "512")
        .env("CBM_MAX_FILE_BYTES", "16777216")
        .env("CBM_RETAIN_TOTAL_MB", "64")
        .env("CBM_RETAIN_PER_FILE_MB", "8")
        .env("CBM_SEMANTIC_ENABLED", "false")
        .env("CBM_LOG_LEVEL", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::isolate(&mut command);
    command
}

fn nonblocking(file: &impl AsRawFd) -> Result<(), String> {
    let descriptor = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

impl Client {
    fn spawn(mut command: Command) -> Result<Self, String> {
        let mut child = command
            .spawn()
            .map_err(|e| format!("Could not start Codebase Memory: {e}"))?;
        let input = child.stdin.take().expect("piped stdin");
        let output = child.stdout.take().expect("piped stdout");
        let errors = child.stderr.take().expect("piped stderr");
        let registration = crate::process::register(child.id());
        let client = Self {
            child,
            input: Some(input),
            output,
            errors,
            pending: Vec::new(),
            diagnostic: Vec::new(),
            next_id: 1,
            broken: false,
            _registration: registration,
        };
        nonblocking(client.input.as_ref().unwrap())?;
        nonblocking(&client.output)?;
        nonblocking(&client.errors)?;
        Ok(client)
    }

    pub(super) fn configure(command: Command, cancel: &AtomicBool) -> Result<(), String> {
        let mut client = Self::spawn(command)?;
        client.input.take();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            client.tick(cancel, deadline)?;
            if let Some(status) = client.child.try_wait().map_err(|e| e.to_string())? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(client.failure("Could not configure Codebase Memory"))
                };
            }
        }
    }

    pub(super) fn connect(command: Command, cancel: &AtomicBool) -> Result<Self, String> {
        let mut client = Self::spawn(command)?;
        client.request("initialize", json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"vorkspace","version":"1"}}), cancel, Duration::from_secs(30))?;
        client.write(
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            cancel,
            Instant::now() + Duration::from_secs(10),
        )?;
        Ok(client)
    }

    fn failure(&self, message: &str) -> String {
        let diagnostic = String::from_utf8_lossy(&self.diagnostic);
        format!(
            "{message}{}{}",
            if diagnostic.is_empty() { "" } else { ": " },
            diagnostic.trim()
        )
    }

    fn tick(&mut self, cancel: &AtomicBool, deadline: Instant) -> Result<(), String> {
        check_cancel(cancel)?;
        if Instant::now() >= deadline {
            return Err("Codebase request timed out".into());
        }
        let mut descriptors = [
            libc::pollfd {
                fd: self.output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.errors.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        unsafe {
            libc::poll(descriptors.as_mut_ptr(), 2, 50);
        }
        let mut buffer = [0u8; 8192];
        // Bounded reads preserve cancellation even if a child continuously logs.
        for _ in 0..8 {
            match self.output.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if self.pending.len() + n > FRAME_LIMIT {
                        return Err("Codebase response exceeded 512 KiB; narrow the query".into());
                    }
                    self.pending.extend_from_slice(&buffer[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        for _ in 0..8 {
            match self.errors.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    self.diagnostic.extend_from_slice(&buffer[..n]);
                    let excess = self.diagnostic.len().saturating_sub(4096);
                    self.diagnostic.drain(..excess);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    fn write(
        &mut self,
        value: &Value,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        if bytes.len() > 32 * 1024 {
            return Err("Codebase request exceeded 32 KiB".into());
        }
        bytes.push(b'\n');
        let mut offset = 0;
        while offset < bytes.len() {
            check_cancel(cancel)?;
            match self
                .input
                .as_mut()
                .ok_or("Codebase connection closed")?
                .write(&bytes[offset..])
            {
                Ok(0) => return Err("Codebase connection closed".into()),
                Ok(n) => offset += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    self.tick(cancel, deadline)?
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    pub(super) fn request(
        &mut self,
        method: &str,
        params: Value,
        cancel: &AtomicBool,
        timeout: Duration,
    ) -> Result<Value, String> {
        let result = self.request_inner(method, params, cancel, timeout);
        self.broken |= result.is_err();
        result
    }

    pub(super) fn idle(&mut self, cancel: &AtomicBool) -> Result<(), String> {
        self.tick(cancel, Instant::now() + Duration::from_secs(1))?;
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            self.pending.drain(..=end);
        }
        if self.child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Err(self.failure("Codebase Memory stopped"));
        }
        Ok(())
    }

    fn request_inner(
        &mut self,
        method: &str,
        params: Value,
        cancel: &AtomicBool,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let deadline = Instant::now() + timeout;
        self.write(
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            cancel,
            deadline,
        )?;
        loop {
            check_cancel(cancel)?;
            while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                let line = self.pending.drain(..=end).collect::<Vec<_>>();
                let response: Value =
                    serde_json::from_slice(&line).map_err(|_| "Invalid Codebase response")?;
                if response.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = response.get("error") {
                    return Err(format!("Codebase: {error}"));
                }
                return response
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "Codebase response has no result".into());
            }
            self.tick(cancel, deadline)?;
            if self.child.try_wait().map_err(|e| e.to_string())?.is_some()
                && !self.pending.contains(&b'\n')
            {
                return Err(self.failure("Codebase Memory stopped"));
            }
        }
    }

    pub(super) fn call(
        &mut self,
        tool: &str,
        arguments: Value,
        cancel: &AtomicBool,
        timeout: Duration,
    ) -> Result<Value, String> {
        let result = self.request(
            "tools/call",
            json!({"name":tool,"arguments":arguments}),
            cancel,
            timeout,
        )?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(result["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"));
        }
        if let Some(value) = result.get("structuredContent") {
            return Ok(value.clone());
        }
        let text = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(script: &str) -> Client {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process::isolate(&mut command);
        Client::spawn(command).unwrap()
    }

    #[test]
    fn truncated_response_fails_at_eof_and_invalidates_connection() {
        let mut client = fixture("read request; printf '{\"id\":1'");
        let error = client
            .request(
                "test",
                json!({}),
                &AtomicBool::new(false),
                Duration::from_secs(5),
            )
            .unwrap_err();
        assert!(error.contains("stopped"), "{error}");
        assert!(client.broken);
    }

    #[test]
    fn oversized_response_is_bounded_and_cancel_bypasses_writes() {
        let mut client = fixture("read request; /usr/bin/head -c 600000 /dev/zero");
        let error = client
            .request(
                "test",
                json!({}),
                &AtomicBool::new(false),
                Duration::from_secs(5),
            )
            .unwrap_err();
        assert!(error.contains("512 KiB"), "{error}");
        assert!(client.pending.len() <= FRAME_LIMIT);
        assert!(client.broken);
        let mut client = fixture("read request");
        let error = client
            .request(
                "test",
                json!({}),
                &AtomicBool::new(true),
                Duration::from_secs(5),
            )
            .unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // EOF cancels synchronous index requests and releases this client's
        // watcher subscription. Never stop another project's shared daemon.
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        crate::process::terminate(&mut self.child);
    }
}
