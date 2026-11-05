//! One owner for a shell's PTY, ordered input, parser replies and child cleanup.
use super::{GridSize, Listener, TermEvent};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use vorkspace_terminal::{Effect, Terminal};

pub(super) struct Pty {
    pub sender: Sender,
    /// Kept open by every foreground query, so its raw descriptor cannot be
    /// reused underneath an in-flight query when the worker exits.
    pub master: Arc<File>,
    pub shell_pid: u32,
}
pub(super) enum Msg {
    Input(Cow<'static, [u8]>),
    Resize {
        size: GridSize,
        cell_width: u16,
        cell_height: u16,
    },
    Shutdown,
}
#[derive(Clone)]
pub(super) struct Sender(Arc<Control>);
const MAX_QUEUED_BYTES: usize = 8 * 1024 * 1024;
// Charge retained Vec capacity until the whole chunk is released, plus an
// allowance for the channel node and pending-queue metadata.
const CHUNK_OVERHEAD: usize = 128;
#[expect(
    clippy::ptr_arg,
    reason = "Queue admission charges retained Vec capacity, which a byte slice cannot expose"
)]
fn retained_bytes(bytes: &Cow<'static, [u8]>) -> usize {
    match bytes {
        Cow::Owned(bytes) => bytes.capacity(),
        Cow::Borrowed(bytes) => bytes.len(),
    }
    .saturating_add(CHUNK_OVERHEAD)
}

struct Control {
    messages: mpsc::Sender<Msg>,
    wake: Mutex<UnixStream>,
    closing: Arc<AtomicBool>,
    queued_bytes: Arc<AtomicUsize>,
}
impl Control {
    fn wake(&self) {
        // A full socket already contains a wakeup. No control operation waits
        // behind PTY output, a large paste or a child that stopped reading.
        let _ = self.wake.lock().unwrap().write(&[1]);
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Release);
        self.wake();
    }
}
fn reserve_bytes(budget: &AtomicUsize, bytes: usize) -> bool {
    let mut current = budget.load(Ordering::Acquire);
    loop {
        let Some(next) = current
            .checked_add(bytes)
            .filter(|next| *next <= MAX_QUEUED_BYTES)
        else {
            return false;
        };
        match budget.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}
impl Sender {
    pub fn send(&self, message: Msg) -> io::Result<()> {
        if matches!(message, Msg::Shutdown) {
            self.0.closing.store(true, Ordering::Release);
        } else {
            if self.0.closing.load(Ordering::Acquire) {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            let bytes = match &message {
                Msg::Input(bytes) => retained_bytes(bytes),
                _ => 0,
            };
            if !reserve_bytes(&self.0.queued_bytes, bytes) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "terminal input queue is full",
                ));
            }
            if self.0.messages.send(message).is_err() {
                self.0.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
        }
        self.0.wake();
        Ok(())
    }
}

/// The kernel wakes idle terminals only for PTY/control readiness or child
/// exit. Deadlines belong exclusively to render holds and final output drain.
struct Waiter {
    queue: File,
    master: i32,
    wake: i32,
    write_enabled: bool,
}
#[derive(Default)]
struct Ready {
    read: bool,
    write: bool,
    wake: bool,
}
fn kevent(ident: usize, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
    libc::kevent {
        ident,
        filter,
        flags,
        fflags,
        data: 0,
        udata: std::ptr::null_mut(),
    }
}
impl Waiter {
    fn new(master: i32, wake: i32, pid: u32) -> io::Result<Self> {
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let queue = unsafe { File::from_raw_fd(fd) };
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let changes = [
            kevent(master as usize, libc::EVFILT_READ, libc::EV_ADD, 0),
            kevent(
                master as usize,
                libc::EVFILT_WRITE,
                libc::EV_ADD | libc::EV_DISABLE,
                0,
            ),
            kevent(wake as usize, libc::EVFILT_READ, libc::EV_ADD, 0),
        ];
        if unsafe {
            libc::kevent(
                fd,
                changes.as_ptr(),
                changes.len() as _,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let child = kevent(
            pid as usize,
            libc::EVFILT_PROC,
            libc::EV_ADD | libc::EV_ONESHOT,
            libc::NOTE_EXIT,
        );
        if unsafe { libc::kevent(fd, &child, 1, std::ptr::null_mut(), 0, std::ptr::null()) } < 0 {
            let error = io::Error::last_os_error();
            // An already exited child is observed by try_wait before blocking.
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(Self {
            queue,
            master,
            wake,
            write_enabled: false,
        })
    }
    fn wait(&mut self, write: bool, timeout: Option<Duration>) -> io::Result<Ready> {
        let change = kevent(
            self.master as usize,
            libc::EVFILT_WRITE,
            if write {
                libc::EV_ENABLE
            } else {
                libc::EV_DISABLE
            },
            0,
        );
        let changed = write != self.write_enabled;
        let timeout = timeout.map(|duration| libc::timespec {
            tv_sec: duration.as_secs() as _,
            tv_nsec: duration.subsec_nanos() as _,
        });
        let mut events: [libc::kevent; 4] = unsafe { std::mem::zeroed() };
        let count = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                if changed { &change } else { std::ptr::null() },
                i32::from(changed),
                events.as_mut_ptr(),
                events.len() as _,
                timeout.as_ref().map_or(std::ptr::null(), |t| t),
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        self.write_enabled = write;
        let mut ready = Ready::default();
        for event in &events[..count as usize] {
            if event.flags & libc::EV_ERROR != 0 {
                return Err(io::Error::from_raw_os_error(event.data as i32));
            }
            if event.ident == self.master as usize {
                if event.filter == libc::EVFILT_READ {
                    ready.read = true;
                }
                if event.filter == libc::EVFILT_WRITE {
                    ready.write = true;
                }
            }
            if event.ident == self.wake as usize && event.filter == libc::EVFILT_READ {
                ready.wake = true;
            }
        }
        Ok(ready)
    }
}
fn nonblocking(fd: i32) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn window_size(size: GridSize, cell_width: u16, cell_height: u16) -> libc::winsize {
    libc::winsize {
        ws_col: size.columns as u16,
        ws_row: size.lines as u16,
        ws_xpixel: (size.columns as u16).saturating_mul(cell_width),
        ws_ypixel: (size.lines as u16).saturating_mul(cell_height),
    }
}
impl Pty {
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: &Path,
        env: HashMap<String, String>,
        size: GridSize,
        term: Arc<Mutex<Terminal>>,
        listener: Listener,
    ) -> io::Result<Self> {
        let mut master = -1;
        let mut slave = -1;
        let mut winsize = window_size(size, 8, 16);
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut winsize,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let master = Arc::new(unsafe { File::from_raw_fd(master) });
        let slave = unsafe { File::from_raw_fd(slave) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        nonblocking(master.as_raw_fd())?;
        let slave_fd = slave.as_raw_fd();
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .envs(env)
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?));
        // Only async-signal-safe syscalls run after fork. A private session
        // gives this terminal its controlling PTY and process-group boundary.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 || libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut mask = std::mem::zeroed();
                libc::sigemptyset(&mut mask);
                libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
                for signal in [
                    libc::SIGHUP,
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGTERM,
                    libc::SIGCHLD,
                ] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        drop(command);
        drop(slave);
        let shell_pid = child.id();
        let (wake, wake_reader) = match UnixStream::pair() {
            Ok(pair) => pair,
            Err(error) => {
                let _ = terminate(&mut child, master.as_raw_fd());
                return Err(error);
            }
        };
        if let Err(error) = wake
            .set_nonblocking(true)
            .and_then(|_| wake_reader.set_nonblocking(true))
        {
            let _ = terminate(&mut child, master.as_raw_fd());
            return Err(error);
        }
        let waiter = match Waiter::new(master.as_raw_fd(), wake_reader.as_raw_fd(), shell_pid) {
            Ok(waiter) => waiter,
            Err(error) => {
                let _ = terminate(&mut child, master.as_raw_fd());
                return Err(error);
            }
        };
        let (messages, rx) = mpsc::channel();
        let closing = Arc::new(AtomicBool::new(false));
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let sender = Sender(Arc::new(Control {
            messages,
            wake: Mutex::new(wake),
            closing: closing.clone(),
            queued_bytes: queued_bytes.clone(),
        }));
        let worker_master = master.clone();
        // If spawning fails, ownership of the child must still reach a reaper.
        let worker_child = Arc::new(Mutex::new(Some(child)));
        let child_for_thread = worker_child.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("terminal-pty".into())
            .spawn(move || {
                let child = child_for_thread.lock().unwrap().take().unwrap();
                Worker {
                    child,
                    master: worker_master,
                    wake: wake_reader,
                    messages: rx,
                    closing,
                    queued_bytes,
                    waiter,
                    term,
                    listener,
                }
                .run();
            })
        {
            if let Some(mut child) = worker_child.lock().unwrap().take() {
                let _ = terminate(&mut child, master.as_raw_fd());
            }
            return Err(error);
        }
        Ok(Self {
            sender,
            master,
            shell_pid,
        })
    }
}

fn child_has_exited(pid: i32) -> io::Result<bool> {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as _,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 {
            return Ok(info.si_pid == pid);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
fn signal_session_jobs(session: i32, signal: i32) {
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return;
    }
    // Slack covers processes born between the size query and enumeration.
    // Enumerate again for escalation, while the unreaped shell pins the SID.
    let mut pids = vec![0i32; count as usize + 64];
    let count = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(pids.as_slice()) as i32,
        )
    };
    pids.truncate(count.max(0) as usize);
    let mut signaled = HashSet::new();
    for member in pids {
        if member <= 1 || unsafe { libc::getsid(member) } != session {
            continue;
        }
        let group = unsafe { libc::getpgid(member) };
        // Use a live member, not the group leader: a job may outlive its
        // leader. A transferred descendant belongs to another session.
        if group > 1
            && !signaled.contains(&group)
            && unsafe { libc::getsid(member) } == session
            && unsafe { libc::getpgid(member) } == group
        {
            unsafe {
                libc::kill(-group, signal);
            }
            signaled.insert(group);
        }
    }
}

fn terminate(child: &mut Child, fd: i32) -> Option<std::process::ExitStatus> {
    // Observe without reaping: a zombie still pins its PID and private
    // session while attached jobs pass their final ownership checks.
    let pid = child.id() as i32;
    if child_has_exited(pid).is_err() {
        return child.try_wait().ok().flatten();
    }
    signal_session_jobs(pid, libc::SIGHUP);
    unsafe {
        libc::tcflush(fd, libc::TCIOFLUSH);
        libc::kill(-pid, libc::SIGHUP);
        libc::kill(pid, libc::SIGHUP);
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        match child_has_exited(pid) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) => return child.try_wait().ok().flatten(),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Re-enumerate and revalidate after the wait: job membership may change
    // during shutdown or outlive the controlling terminal's foreground link.
    signal_session_jobs(pid, libc::SIGKILL);
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let killed = child.kill();
    // macOS can keep a SIGKILLed process in exit while pending PTY echo waits
    // to drain. We cannot wait for reaping before releasing that queue: the
    // last master is intentionally kept alive for foreground queries.
    unsafe {
        libc::tcflush(fd, libc::TCIOFLUSH);
    }
    if killed.is_err() {
        return child.try_wait().ok().flatten();
    }
    child.wait().ok()
}

struct Worker {
    child: Child,
    master: Arc<File>,
    wake: UnixStream,
    messages: mpsc::Receiver<Msg>,
    closing: Arc<AtomicBool>,
    queued_bytes: Arc<AtomicUsize>,
    waiter: Waiter,
    term: Arc<Mutex<Terminal>>,
    listener: Listener,
}
impl Worker {
    fn run(self) {
        let Self {
            mut child,
            master,
            mut wake,
            messages,
            closing,
            queued_bytes,
            mut waiter,
            term,
            listener,
        } = self;
        let fd = master.as_raw_fd();
        let mut pending: VecDeque<Cow<'static, [u8]>> = VecDeque::new();
        let mut written = 0;
        let mut eof = false;
        let mut exit_at = None;
        let mut buf = [0u8; 16 * 1024];
        loop {
            if closing.load(Ordering::Acquire) {
                break;
            }
            // Bound each iteration so child output and shutdown remain responsive
            // while the user pastes or a full-screen program receives key repeats.
            let mut commands_full = true;
            for _ in 0..64 {
                match messages.try_recv() {
                    Ok(Msg::Input(bytes)) => {
                        if !bytes.is_empty() {
                            pending.push_back(bytes);
                        } else {
                            queued_bytes.fetch_sub(retained_bytes(&bytes), Ordering::AcqRel);
                        }
                    }
                    Ok(Msg::Resize {
                        size,
                        cell_width,
                        cell_height,
                    }) => {
                        let winsize = window_size(size, cell_width, cell_height);
                        unsafe {
                            libc::ioctl(fd, libc::TIOCSWINSZ, &winsize);
                        }
                    }
                    Ok(Msg::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
                        closing.store(true, Ordering::Release);
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        commands_full = false;
                        break;
                    }
                }
            }
            if closing.load(Ordering::Acquire) {
                break;
            }
            if exit_at.is_none() {
                match child_has_exited(child.id() as i32) {
                    Ok(true) => {
                        exit_at = Some(Instant::now());
                    }
                    Ok(false) => {}
                    Err(_) => {
                        eof = true;
                    }
                }
            }
            let timeout = if commands_full {
                Some(Duration::ZERO)
            } else {
                let hold = term.lock().unwrap().render_hold_remaining();
                let drain = exit_at
                    .map(|at: Instant| Duration::from_millis(200).saturating_sub(at.elapsed()));
                match (hold, drain) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            };
            let ready = match waiter.wait(!pending.is_empty(), timeout) {
                Ok(ready) => ready,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            if ready.wake {
                while matches!(wake.read(&mut buf), Ok(n) if n > 0) {}
            }
            if closing.load(Ordering::Acquire) {
                break;
            }
            if ready.read {
                for _ in 0..16 {
                    if closing.load(Ordering::Acquire) {
                        break;
                    }
                    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                    if n > 0 {
                        let effects = {
                            let mut term = term.lock().unwrap();
                            term.write(&buf[..n as usize]);
                            term.take_effects()
                        };
                        for effect in effects {
                            match effect {
                                Effect::PtyWrite(bytes) => {
                                    if !bytes.is_empty() {
                                        let bytes = Cow::Owned(bytes);
                                        if !reserve_bytes(&queued_bytes, retained_bytes(&bytes)) {
                                            term.lock()
                                                .unwrap()
                                                .write(b"\r\n[terminal reply queue is full]");
                                            closing.store(true, Ordering::Release);
                                            break;
                                        }
                                        pending.push_back(bytes);
                                    }
                                }
                                Effect::ClipboardStore(text) => {
                                    listener.send_event(TermEvent::ClipboardStore(text))
                                }
                            }
                        }
                        listener.send_event(TermEvent::Wakeup);
                    } else if n == 0 {
                        eof = true;
                        break;
                    } else {
                        let error = io::Error::last_os_error();
                        if error.kind() == io::ErrorKind::Interrupted {
                            continue;
                        }
                        if error.kind() != io::ErrorKind::WouldBlock {
                            eof = true;
                        }
                        break;
                    }
                }
            }
            if ready.write {
                let mut budget = 64 * 1024;
                while let Some(bytes) = pending.front() {
                    if budget == 0 {
                        break;
                    }
                    let slice = &bytes[written..];
                    let n =
                        unsafe { libc::write(fd, slice.as_ptr().cast(), slice.len().min(budget)) };
                    if n > 0 {
                        written += n as usize;
                        budget -= n as usize;
                        if written == bytes.len() {
                            let bytes = pending.pop_front().unwrap();
                            queued_bytes.fetch_sub(retained_bytes(&bytes), Ordering::AcqRel);
                            written = 0;
                        }
                    } else {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::WouldBlock
                            && error.kind() != io::ErrorKind::Interrupted
                        {
                            eof = true;
                        }
                        break;
                    }
                }
            }
            if term.lock().unwrap().expire_render_hold() {
                listener.send_event(TermEvent::Wakeup);
            }
            // Drain the child's final bytes. Descendants can keep the slave open;
            // a dead login shell must still complete its terminal lifecycle.
            if eof || exit_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(200)) {
                break;
            }
        }
        // Keep the shell unreaped through final output drain and job cleanup,
        // including normal exit. Its PID pins the session against reuse.
        let exited = terminate(&mut child, fd);
        if let Some(status) = exited {
            listener.send_event(TermEvent::ChildExit(status));
        } else {
            listener.send_event(TermEvent::Exit);
        }
    }
}
