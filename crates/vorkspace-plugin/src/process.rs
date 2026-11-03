use super::*;
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
static CHILDREN: AtomicUsize = AtomicUsize::new(0);
const MAX_CHILDREN: usize = 8;
struct Admission<'a>(&'a AtomicUsize);
impl<'a> Admission<'a> {
    fn acquire(counter: &'a AtomicUsize) -> Result<Self, String> {
        // Keep this on stable atomic primitives shared by local and CI toolchains.
        let mut observed = counter.load(Ordering::Acquire);
        while observed < MAX_CHILDREN {
            match counter.compare_exchange_weak(
                observed,
                observed + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(Self(counter)),
                Err(current) => observed = current,
            }
        }
        Err("Plugins are busy. Try again shortly.".into())
    }
}
impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Connection {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    errors: ChildStderr,
    pending: Vec<u8>,
    deadline: Instant,
}
impl Drop for Connection {
    fn drop(&mut self) {
        // Each call has a dedicated group. Unknown remote effects are never replayed.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}
fn nonblocking(fd: i32) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
impl Connection {
    fn wait(&mut self, fd: i32, events: i16) -> Result<(), String> {
        loop {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("Plugin timed out; the request was not retried".into());
            }
            let mut fds = [
                libc::pollfd {
                    fd,
                    events,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.errors.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let result =
                unsafe { libc::poll(fds.as_mut_ptr(), 2, remaining.as_millis().min(1000) as i32) };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.to_string());
            }
            let mut discarded = [0; 4096];
            if fds[1].revents & libc::POLLIN != 0 {
                let _ = self.errors.read(&mut discarded);
            }
            if fds[0].revents & (events | libc::POLLHUP | libc::POLLERR) != 0 {
                return Ok(());
            }
        }
    }
    fn write(&mut self, frame: &impl Serialize) -> Result<(), String> {
        let mut data = serde_json::to_vec(frame).map_err(|e| e.to_string())?;
        if data.len() > MAX_FRAME {
            return Err("Plugin message exceeded 8 MiB".into());
        }
        data.push(b'\n');
        let mut offset = 0;
        while offset < data.len() {
            self.wait(self.input.as_raw_fd(), libc::POLLOUT)?;
            match self.input.write(&data[offset..]) {
                Ok(0) => return Err("Plugin closed its input".into()),
                Ok(n) => offset += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
    fn read(&mut self) -> Result<Value, String> {
        loop {
            if let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                if end > MAX_FRAME {
                    return Err("Plugin message exceeded 8 MiB".into());
                }
                let line = self.pending.drain(..=end).collect::<Vec<_>>();
                return serde_json::from_slice(&line)
                    .map_err(|e| format!("Invalid plugin message: {e}"));
            }
            if self.pending.len() > MAX_FRAME {
                return Err("Plugin message exceeded 8 MiB".into());
            }
            self.wait(self.output.as_raw_fd(), libc::POLLIN)?;
            let mut bytes = [0; 8192];
            match self.output.read(&mut bytes) {
                Ok(0) => {
                    return Err(
                        "Plugin exited before returning a result; the request was not retried"
                            .into(),
                    );
                }
                Ok(n) => self.pending.extend_from_slice(&bytes[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    fn response(
        &mut self,
        id: u64,
        manifest: Option<&Manifest>,
        host: &mut impl FnMut(&str, &Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        loop {
            let value = self.read()?;
            if value.get("method").is_some() {
                let request: Request = serde_json::from_value(value).map_err(|e| e.to_string())?;
                let method = request
                    .method
                    .strip_prefix("host.")
                    .ok_or("Plugin requested an unknown host method")?;
                let result =
                    if request.jsonrpc != "2.0" || !manifest.is_some_and(|m| m.permits(method)) {
                        Err("Plugin has not been granted this host capability".into())
                    } else {
                        host(method, &request.params)
                    };
                self.write(&Response::new(request.id, result))?;
            } else {
                return serde_json::from_value::<Response>(value)
                    .map_err(|e| e.to_string())?
                    .value(id);
            }
        }
    }
}
/// One bounded, isolated process per call. Mutable plugin state is explicit in
/// its private directory; closing a view cannot strand an implicit subprocess.
pub fn call(
    executable: &Path,
    expected_id: Option<&str>,
    context: &Context,
    method: &str,
    params: Value,
    timeout: Duration,
    mut host: impl FnMut(&str, &Value) -> Result<Value, String>,
) -> Result<(Manifest, Value), String> {
    let _admission = Admission::acquire(&CHILDREN)?;
    let mut child = Command::new(executable)
        .current_dir(&context.cwd)
        .env_remove("VORKSPACE_APP_TOKEN")
        .env_remove("VORKSPACE_VISUAL_TOKEN")
        .env_remove("VORKSPACE_DEV_RELOAD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    let input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let errors = child.stderr.take().unwrap();
    let mut connection = Connection {
        child,
        input,
        output,
        errors,
        pending: Vec::new(),
        deadline: Instant::now() + timeout,
    };
    nonblocking(connection.input.as_raw_fd())?;
    nonblocking(connection.output.as_raw_fd())?;
    nonblocking(connection.errors.as_raw_fd())?;
    connection.write(&Request::new(
        1,
        "initialize",
        serde_json::to_value(context).unwrap(),
    ))?;
    let manifest: Manifest = serde_json::from_value(connection.response(1, None, &mut host)?)
        .map_err(|e| e.to_string())?;
    manifest.validate()?;
    if expected_id.is_some_and(|id| id != manifest.id) {
        return Err("Plugin identity changed; reinstall it before continuing".into());
    }
    connection.write(&Request::new(2, method, params))?;
    let result = connection.response(2, Some(&manifest), &mut host)?;
    Ok((manifest, result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn executable(directory: &Path, body: &str) -> PathBuf {
        let path = directory.join("plugin.py");
        std::fs::write(&path,format!("#!/usr/bin/env python3\nimport sys,json,time,os\ninit=json.loads(sys.stdin.readline())\ndef send(id,result):\n print(json.dumps({{'jsonrpc':'2.0','id':id,'result':result}}),flush=True)\nsend(1,{{'protocol_version':1,'id':'fixture','name':'Fixture','version':'1','capabilities':[],'ui':True}})\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    #[test]
    fn concurrent_admission_bounds_processes_and_releases_every_slot() {
        let counter = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(MAX_CHILDREN * 2);
        let mut held = std::thread::scope(|scope| {
            let handles = (0..MAX_CHILDREN * 2)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        Admission::acquire(&counter).ok()
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(held.len(), MAX_CHILDREN);
        assert_eq!(counter.load(Ordering::Acquire), MAX_CHILDREN);
        assert!(Admission::acquire(&counter).is_err());
        drop(held.pop());
        held.push(Admission::acquire(&counter).unwrap());
        drop(held);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }
    #[test]
    fn round_trip_correlates_responses_and_refuses_identity_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(
            dir.path(),
            "request=json.loads(sys.stdin.readline())\nsend(request['id'],request['params'])",
        );
        let context = Context {
            data_dir: dir.path().into(),
            cwd: dir.path().into(),
        };
        assert_eq!(
            call(
                &path,
                Some("fixture"),
                &context,
                "echo",
                serde_json::json!({"text":"🦊"}),
                Duration::from_secs(3),
                |_, _| unreachable!()
            )
            .unwrap()
            .1,
            serde_json::json!({"text":"🦊"})
        );
        assert!(
            call(
                &path,
                Some("another"),
                &context,
                "echo",
                Value::Null,
                Duration::from_secs(3),
                |_, _| unreachable!()
            )
            .unwrap_err()
            .contains("identity")
        );
    }
    #[test]
    fn undeclared_native_capabilities_never_reach_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(
            dir.path(),
            "request=json.loads(sys.stdin.readline())\nprint(json.dumps({'jsonrpc':'2.0','id':1001,'method':'host.audio.start','params':{}}),flush=True)\nresult=json.loads(sys.stdin.readline())\nsend(request['id'],result['error']['message'])",
        );
        let context = Context {
            data_dir: dir.path().into(),
            cwd: dir.path().into(),
        };
        let (_, result) = call(
            &path,
            Some("fixture"),
            &context,
            "echo",
            Value::Null,
            Duration::from_secs(3),
            |_, _| panic!("Capability was not admitted"),
        )
        .unwrap();
        assert!(result.as_str().unwrap().contains("not been granted"));
    }
    #[test]
    fn blocked_stdin_is_bounded_and_never_replayed() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(dir.path(), "time.sleep(20)");
        let context = Context {
            data_dir: dir.path().into(),
            cwd: dir.path().into(),
        };
        let error = call(
            &path,
            Some("fixture"),
            &context,
            "echo",
            serde_json::json!({"body":"x".repeat(1024*1024)}),
            Duration::from_millis(750),
            |_, _| unreachable!(),
        )
        .unwrap_err();
        assert!(error.contains("timed out"));
        assert!(error.contains("not retried"));
    }
    #[test]
    fn unsafe_plugin_ids_and_versions_fail_before_use() {
        for id in ["../escape", "", ".hidden", "a..b", "UPPER"] {
            assert!(!valid_id(id));
        }
        let mut manifest = Manifest {
            protocol_version: VERSION + 1,
            id: "safe".into(),
            name: "Safe".into(),
            version: "1".into(),
            capabilities: vec![],
            ui: true,
            connector: false,
        };
        assert!(manifest.validate().is_err());
        manifest.protocol_version = VERSION;
        manifest.capabilities.push("app.execute".into());
        assert!(manifest.validate().is_err());
        let mut input = std::io::Cursor::new(vec![b'x'; MAX_FRAME + 2]);
        assert!(
            read_frame::<Value>(&mut input)
                .unwrap_err()
                .contains("framing")
        );
    }
}
