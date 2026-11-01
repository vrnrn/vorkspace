//! The contract between the app and its clipboard helper, which outlives the app
//! so that copies keep being recorded after Quit. Only control messages cross the
//! socket: clipboard contents stay in the helper.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::{net::UnixStream, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command as Process, Stdio},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, UNIX_EPOCH},
};

const PROTOCOL: u32 = 1;
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);
const CONNECT_ATTEMPTS: usize = 60;
const CONNECT_DELAY: Duration = Duration::from_millis(50);
const RETRY_DELAY: Duration = Duration::from_millis(500);
const REFUSED_DELAY: Duration = Duration::from_secs(2);
const FAILED_START_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Configure {
        recording: bool,
        icon: bool,
    },
    /// How to reopen this exact app, including the data it was started with.
    Relaunch {
        program: String,
        arguments: Vec<String>,
    },
    Clear,
    Toggle,
    Shutdown,
}

/// Panel buttons that belong to the app.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    Settings,
    Composer,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Notice {
    Hello { protocol: u32, modified: u128 },
    Error(Option<String>),
    Action(Action),
}

impl Notice {
    /// What the helper says first, so the app can replace an outdated one.
    pub fn hello(executable: &Path) -> Self {
        Self::Hello {
            protocol: PROTOCOL,
            modified: modified(executable),
        }
    }
}

fn modified(path: &Path) -> u128 {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |age| age.as_nanos())
}

pub fn send(mut stream: &UnixStream, message: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    stream.write_all(&line)
}

/// One message per line. `None` is end of stream, or a message from the wrong
/// protocol, which is treated the same way because nothing can follow it.
pub fn receive<T: DeserializeOwned>(reader: &mut impl BufRead) -> Option<T> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => serde_json::from_str(&line).ok(),
    }
}

/// The helper is its own nested app. Sharing the app's bundle identity would make
/// macOS treat Vorkspace as already running, so the Dock could never launch it.
pub fn helper_executable(app: &Path) -> PathBuf {
    let directory = app.parent().unwrap_or(Path::new("."));
    // Canonical, because the helper finds the app by walking up this exact path.
    directory
        .join("../Helpers/vorkspace Clipboard.app/Contents/MacOS/vorkspace-clipboard")
        .canonicalize()
        .unwrap_or_else(|_| directory.join("vorkspace-clipboard"))
}

/// Each install gets its own helper, so a development build running beside the
/// installed app never replaces the other's helper.
pub fn socket_path(helper: &Path) -> PathBuf {
    let hash = helper
        .to_string_lossy()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    // The per-user temporary directory keeps the path inside the socket length limit.
    std::env::temp_dir().join(format!("vorkspace-clipboard-{hash:016x}.sock"))
}

/// A bundle reopens through Launch Services; a bare Cargo binary runs directly.
pub fn relaunch_command(app: &Path, arguments: Vec<String>) -> Command {
    let bundle = app
        .ancestors()
        .nth(3)
        .filter(|path| path.extension().is_some_and(|extension| extension == "app"));
    match bundle {
        Some(bundle) => {
            let mut open = vec![bundle.to_string_lossy().into_owned()];
            if !arguments.is_empty() {
                open.push("--args".into());
                open.extend(arguments);
            }
            Command::Relaunch {
                program: "/usr/bin/open".into(),
                arguments: open,
            }
        }
        None => Command::Relaunch {
            program: app.to_string_lossy().into_owned(),
            arguments,
        },
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Desired {
    recording: bool,
    icon: bool,
}
#[derive(Default)]
struct Shared {
    desired: Mutex<Desired>,
    wake: Condvar,
    stream: Mutex<Option<UnixStream>>,
    clear_pending: Mutex<bool>,
}

/// The app's side: keeps one helper running and configured while it is wanted,
/// replacing one that crashed or came from an older build.
pub struct Supervisor {
    shared: Arc<Shared>,
    socket: PathBuf,
}

impl Supervisor {
    pub fn start(
        helper: PathBuf,
        socket: PathBuf,
        relaunch: Command,
        notify: impl Fn(Notice) + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        let owned = shared.clone();
        std::thread::Builder::new()
            .name("clipboard-helper".into())
            .spawn({
                let socket = socket.clone();
                move || supervise(&owned, &helper, &socket, &relaunch, notify)
            })
            .expect("start the clipboard helper supervisor");
        Self { shared, socket }
    }

    pub fn configure(&self, recording: bool, icon: bool) {
        *self.shared.desired.lock().unwrap() = Desired { recording, icon };
        self.shared.wake.notify_all();
        if !recording {
            // Stopping recording drops the history a queued clear was aimed at.
            *self.shared.clear_pending.lock().unwrap() = false;
        }
        let command = Command::Configure { recording, icon };
        if !self.send(command.clone()) && !recording {
            let shared = self.shared.clone();
            let socket = self.socket.clone();
            std::thread::spawn(move || retire(&shared, &socket, command));
        }
    }

    pub fn send(&self, command: Command) -> bool {
        self.shared
            .stream
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|stream| send(stream, &command).is_ok())
    }

    /// Clearing is a promise to the user, so it waits out a reconnect.
    pub fn clear(&self) {
        let stream = self.shared.stream.lock().unwrap();
        if !stream
            .as_ref()
            .is_some_and(|stream| send(stream, &Command::Clear).is_ok())
        {
            *self.shared.clear_pending.lock().unwrap() = true;
        }
    }

    pub fn connected(&self) -> bool {
        self.shared.stream.lock().unwrap().is_some()
    }
}

/// Recording off means no helper at all, even one this app never connected to.
/// A helper still closing its last client refuses this one, so the command only
/// counts once Hello proves it was accepted.
fn retire(shared: &Shared, socket: &Path, command: Command) {
    for _ in 0..CONNECT_ATTEMPTS {
        let Ok(stream) = UnixStream::connect(socket) else {
            return;
        };
        let accepted = receive::<Notice>(&mut BufReader::new(&stream)).is_some();
        if shared.desired.lock().unwrap().recording {
            return;
        }
        if accepted {
            let _ = send(&stream, &command);
            return;
        }
        std::thread::sleep(CONNECT_DELAY);
    }
}

fn supervise(
    shared: &Shared,
    helper: &Path,
    socket: &Path,
    relaunch: &Command,
    notify: impl Fn(Notice),
) {
    loop {
        drop(
            shared
                .wake
                .wait_while(shared.desired.lock().unwrap(), |desired| !desired.recording)
                .unwrap(),
        );
        let Some(stream) = connect(helper, socket) else {
            notify(Notice::Error(Some(
                "The clipboard helper did not start.".into(),
            )));
            std::thread::sleep(FAILED_START_DELAY);
            continue;
        };
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let mut reader = BufReader::new(&stream);
        let Some(Notice::Hello {
            protocol,
            modified: built,
        }) = receive(&mut reader)
        else {
            // Another copy of the app holds this helper.
            std::thread::sleep(REFUSED_DELAY);
            continue;
        };
        if protocol != PROTOCOL || built != modified(helper) {
            let _ = send(&stream, &Command::Shutdown);
            // A panel action may already be on its way. It outlives the old helper.
            while let Some(notice) = receive(&mut reader) {
                if matches!(notice, Notice::Action(_)) {
                    notify(notice);
                }
            }
            continue;
        }
        {
            // Under this lock, a configure either lands before publication and is
            // sent here, or sees the stream and sends itself.
            let mut published = shared.stream.lock().unwrap();
            let Desired { recording, icon } = *shared.desired.lock().unwrap();
            if send(&stream, &Command::Configure { recording, icon }).is_ok()
                && send(&stream, relaunch).is_ok()
            {
                *published = stream.try_clone().ok();
                let mut clear = shared.clear_pending.lock().unwrap();
                if *clear && send(&stream, &Command::Clear).is_ok() {
                    *clear = false;
                }
            }
        }
        notify(Notice::Error(None));
        while let Some(notice) = receive(&mut reader) {
            notify(notice);
        }
        *shared.stream.lock().unwrap() = None;
        std::thread::sleep(RETRY_DELAY);
    }
}

fn connect(helper: &Path, socket: &Path) -> Option<UnixStream> {
    if let Ok(stream) = UnixStream::connect(socket) {
        return Some(stream);
    }
    let mut child = Process::new(helper)
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // It outlives this app, so the runner's stdin hook can't apply to a relaunch.
        .env_remove("VORKSPACE_DEV_RELOAD")
        // Its own group keeps it alive when the app's terminal group is signalled.
        .process_group(0)
        .spawn()
        .ok()?;
    std::thread::spawn(move || child.wait());
    (0..CONNECT_ATTEMPTS).find_map(|_| {
        std::thread::sleep(CONNECT_DELAY);
        UnixStream::connect(socket).ok()
    })
}

/// What the helper does next. Pure, so each lifecycle overlap has a test.
#[derive(Debug, Default)]
pub struct Control {
    recording: bool,
    client: bool,
    pending: Option<Action>,
}

#[derive(Debug, PartialEq)]
pub enum Step {
    Deliver(Action),
    LaunchApp,
    Exit,
}

impl Control {
    pub fn configure(&mut self, recording: bool) -> Option<Step> {
        self.recording = recording;
        (!recording).then_some(Step::Exit)
    }

    pub fn connected(&mut self) -> Option<Step> {
        self.client = true;
        self.pending.take().map(Step::Deliver)
    }

    /// An app that left before configuring anything wanted no recording.
    pub fn disconnected(&mut self) -> Option<Step> {
        self.client = false;
        (!self.recording).then_some(Step::Exit)
    }

    pub fn action(&mut self, action: Action) -> Step {
        match (self.client, action) {
            (true, action) => Step::Deliver(action),
            (false, Action::Quit) => Step::Exit,
            (false, action) => {
                self.pending = Some(action);
                Step::LaunchApp
            }
        }
    }

    /// A helper the app never reached has no reason to stay.
    pub fn abandoned(&self) -> bool {
        !self.client && !self.recording
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_is_the_only_reason_to_run() {
        let mut control = Control::default();
        control.connected();
        assert_eq!(control.configure(true), None);
        assert_eq!(control.disconnected(), None);
        control.connected();
        assert_eq!(control.configure(false), Some(Step::Exit));
    }

    #[test]
    fn an_app_that_left_before_configuring_takes_the_helper_with_it() {
        let mut control = Control::default();
        control.connected();
        assert_eq!(control.disconnected(), Some(Step::Exit));
    }

    #[test]
    fn panel_actions_wait_for_the_app_they_launch() {
        let mut control = Control::default();
        control.configure(true);
        assert_eq!(control.action(Action::Settings), Step::LaunchApp);
        assert_eq!(control.connected(), Some(Step::Deliver(Action::Settings)));
        assert_eq!(control.connected(), None);
        assert_eq!(
            control.action(Action::Composer),
            Step::Deliver(Action::Composer)
        );
    }

    #[test]
    fn quit_goes_through_the_app_when_it_is_open() {
        let mut control = Control::default();
        control.configure(true);
        assert_eq!(control.action(Action::Quit), Step::Exit);
        control.connected();
        assert_eq!(control.action(Action::Quit), Step::Deliver(Action::Quit));
    }

    #[test]
    fn only_an_idle_helper_counts_as_abandoned() {
        let mut control = Control::default();
        assert!(control.abandoned());
        control.configure(true);
        assert!(!control.abandoned());
    }

    #[test]
    fn a_nested_helper_is_preferred_over_a_sibling() {
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("MacOS/vorkspace-gpui");
        assert_eq!(
            helper_executable(&app),
            directory.path().join("MacOS/vorkspace-clipboard")
        );
        let nested = directory
            .path()
            .join("MacOS/../Helpers/vorkspace Clipboard.app/Contents/MacOS");
        std::fs::create_dir_all(directory.path().join("MacOS")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("vorkspace-clipboard"), "").unwrap();
        assert_eq!(
            helper_executable(&app),
            nested.join("vorkspace-clipboard").canonicalize().unwrap()
        );
    }

    #[test]
    fn a_relaunch_keeps_the_arguments_the_app_started_with() {
        let arguments = vec!["--vorkspace-db".to_owned(), "/data/store.db".to_owned()];
        assert_eq!(
            relaunch_command(
                Path::new("/Applications/vorkspace.app/Contents/MacOS/vorkspace-gpui"),
                arguments.clone()
            ),
            Command::Relaunch {
                program: "/usr/bin/open".into(),
                arguments: vec![
                    "/Applications/vorkspace.app".into(),
                    "--args".into(),
                    "--vorkspace-db".into(),
                    "/data/store.db".into(),
                ],
            }
        );
        assert_eq!(
            relaunch_command(
                Path::new("/Users/me/project/target/native-dev/vorkspace-gpui"),
                arguments.clone()
            ),
            Command::Relaunch {
                program: "/Users/me/project/target/native-dev/vorkspace-gpui".into(),
                arguments,
            }
        );
    }

    #[test]
    fn each_install_gets_its_own_socket() {
        let first = socket_path(Path::new("/Applications/vorkspace.app/helper"));
        assert_eq!(
            first,
            socket_path(Path::new("/Applications/vorkspace.app/helper"))
        );
        assert_ne!(first, socket_path(Path::new("/tmp/vorkspace.app/helper")));
        assert!(first.to_string_lossy().len() < 104);
    }
}
