//! Child process policy: group isolation and termination.
use std::process::Command;

/// Apply scheduling hints before exec so descendants inherit them.
/// Best effort: a refused hint must not prevent a provider from launching.
pub fn configure_child(command: &mut Command) {
    {
        use std::os::unix::process::CommandExt;
        // Only system calls here: the post-fork child must not allocate or lock.
        unsafe {
            command.pre_exec(|| {
                // A GUI executor's worker may block signals. exec preserves
                // that mask; Tokio-based provider children then never receive
                // SIGCHLD and their command sessions never finish. Give the
                // provider the ordinary process mask before it starts.
                let mut signals = std::mem::zeroed();
                libc::sigemptyset(&mut signals);
                if libc::sigprocmask(libc::SIG_SETMASK, &signals, std::ptr::null_mut()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let priority = libc::getpriority(libc::PRIO_PROCESS, 0).max(10);
                libc::setpriority(libc::PRIO_PROCESS, 0, priority);
                setiopolicy_np(0, 0, 3); // DISK, PROCESS, THROTTLE
                Ok(())
            });
        }
    }
    let _ = command;
}

unsafe extern "C" {
    fn setiopolicy_np(kind: i32, scope: i32, policy: i32) -> i32;
}

/// New native provider processes use a separate group for cancellation.
pub fn isolate(command: &mut Command) {
    configure_child(command);
    command.env("VORKSPACE_HARNESS_PARENT", std::process::id().to_string());
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

/// Called only by the provider worker. Always reap the immediate child.
pub fn terminate(child: &mut std::process::Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
/// A provider child that belongs to a session, for the running-work list.
#[derive(Clone, Debug)]
pub struct Agent {
    pub session: i64,
    pub pid: u32,
    /// Birth stamp at spawn; a reused PID no longer matches it.
    pub identity: Option<String>,
}
fn active() -> &'static Mutex<HashMap<u32, Option<Agent>>> {
    static ACTIVE: OnceLock<Mutex<HashMap<u32, Option<Agent>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}
thread_local! {
    static OWNER: Cell<Option<i64>> = const { Cell::new(None) };
}
/// Children spawned by `run` on this thread are attributed to `session`.
pub fn owned_by<T>(session: i64, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<i64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OWNER.set(self.0);
        }
    }
    let _restore = Restore(OWNER.replace(Some(session)));
    run()
}
/// Live provider children that belong to a session.
pub fn agents() -> Vec<Agent> {
    active()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .flatten()
        .cloned()
        .collect()
}
pub(crate) fn owner() -> Option<i64> {
    OWNER.get()
}
pub(crate) fn mark_idle(pid: u32, idle: bool) {
    let mut processes = active().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(slot) = processes.get_mut(&pid) {
        *slot = if idle {
            None
        } else {
            OWNER.get().map(|session| Agent {
                session,
                pid,
                identity: identity(pid),
            })
        };
    }
}
pub struct Registration(u32);
pub fn register(pid: u32) -> Registration {
    let agent = OWNER.get().map(|session| Agent {
        session,
        pid,
        identity: identity(pid),
    });
    active()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(pid, agent);
    Registration(pid)
}
impl Drop for Registration {
    fn drop(&mut self) {
        active()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}
/// Explicit native-app quit hook. Run off the UI thread and wait briefly for the
/// workers to reap their children, rather than leaving isolated provider groups.
pub fn shutdown_all() {
    let pids: Vec<_> = active()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .copied()
        .collect();
    for pid in pids {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !active()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

pub fn identity(pid: u32) -> Option<String> {
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of_val(&info) as i32;
        if libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut _,
            size,
        ) != size
        {
            return None;
        }
        Some(format!(
            "{}:{}:{}",
            pid, info.pbi_start_tvsec, info.pbi_start_tvusec
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_child_does_not_inherit_executor_signal_mask() {
        const CHILD: &str = "VORKSPACE_SIGNAL_REGRESSION_CHILD";
        unsafe {
            let mut mask = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut mask);
            if std::env::var_os(CHILD).is_some() {
                assert_eq!(libc::sigismember(&mask, libc::SIGCHLD), 0);
                return;
            }
            let mut blocked = std::mem::zeroed();
            libc::sigemptyset(&mut blocked);
            libc::sigaddset(&mut blocked, libc::SIGCHLD);
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, std::ptr::null_mut()),
                0
            );
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "process::tests::provider_child_does_not_inherit_executor_signal_mask",
                    "--nocapture",
                ])
                .env(CHILD, "1");
            isolate(&mut command);
            let result = command.output();
            libc::pthread_sigmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
            let output = result.unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
}
