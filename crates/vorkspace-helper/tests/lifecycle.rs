//! Drives the real helper process over its socket. Nothing here records a copy:
//! contents never cross this boundary, so lifecycle is what's visible from outside.
use std::{
    io::BufReader,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command as Process},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use vorkspace_helper::{Action, Command, Notice, Supervisor, receive, send};

const HELPER: &str = env!("CARGO_BIN_EXE_vorkspace-helper");
const TIMEOUT: Duration = Duration::from_secs(20);

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(helper: &Path, socket: &Path) -> Running {
    Running(
        Process::new(helper)
            .arg("--socket")
            .arg(socket)
            .spawn()
            .unwrap(),
    )
}

fn until(mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < TIMEOUT, "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn exits(helper: &mut Running) {
    until(|| helper.0.try_wait().unwrap().is_some());
}

fn read_hello(stream: &UnixStream) -> Option<Notice> {
    // Preserve subsequent notices when this temporary reader is dropped.
    receive(&mut BufReader::with_capacity(1, stream))
}

fn connect(socket: &Path) -> (UnixStream, Notice) {
    let mut found = None;
    until(|| {
        found = UnixStream::connect(socket).ok().and_then(|stream| {
            let hello = read_hello(&stream)?;
            Some((stream, hello))
        });
        found.is_some()
    });
    found.unwrap()
}

#[test]
fn reading_hello_preserves_an_immediately_following_notice() {
    let (sender, receiver) = UnixStream::pair().unwrap();
    let hello = Notice::Hello {
        protocol: 1,
        modified: 2,
    };
    let status = Notice::Error(Some("next notice".into()));
    send(&sender, &hello).unwrap();
    send(&sender, &status).unwrap();
    assert_eq!(read_hello(&receiver), Some(hello));
    assert_eq!(
        receive::<Notice>(&mut BufReader::new(&receiver)),
        Some(status)
    );
}

fn relaunch() -> Command {
    vorkspace_helper::relaunch_command(Path::new("/tmp/vorkspace-gpui"), Vec::new())
}

fn socket(directory: &tempfile::TempDir) -> PathBuf {
    directory.path().join("c.sock")
}

#[test]
fn the_helper_outlives_the_app_while_recording_and_exits_when_turned_off() {
    let directory = tempfile::tempdir().unwrap();
    let socket = socket(&directory);
    let mut helper = start(Path::new(HELPER), &socket);
    let (app, _) = connect(&socket);
    send(
        &app,
        &Command::Configure {
            recording: true,
            icon: false,
            dictation: Default::default(),
        },
    )
    .unwrap();
    drop(app);
    std::thread::sleep(Duration::from_millis(300));
    assert!(helper.0.try_wait().unwrap().is_none(), "kept recording");
    let (app, _) = connect(&socket);
    send(
        &app,
        &Command::Configure {
            recording: false,
            icon: false,
            dictation: Default::default(),
        },
    )
    .unwrap();
    exits(&mut helper);
}

#[test]
fn dictation_keeps_the_helper_alive_with_history_off_and_survives_reconnect() {
    use vorkspace_helper::{DictationConfig, DictationPhase};
    let directory = tempfile::tempdir().unwrap();
    let socket = socket(&directory);
    let mut helper = start(Path::new(HELPER), &socket);
    let (app, _) = connect(&socket);
    app.set_read_timeout(Some(TIMEOUT)).unwrap();
    send(
        &app,
        &Command::Configure {
            recording: false,
            icon: false,
            // No global shortcut or microphone is used by this process fixture.
            dictation: DictationConfig {
                enabled: true,
                shortcut: String::new(),
                words: "work space → Vorkspace".into(),
                ..Default::default()
            },
        },
    )
    .unwrap();
    let mut reader = BufReader::new(&app);
    loop {
        if let Notice::Dictation(status) = receive::<Notice>(&mut reader).expect("helper status")
            && status.phase != DictationPhase::Off
        {
            break;
        }
    }
    drop(reader);
    drop(app);
    let (reconnected, _) = connect(&socket);
    reconnected.set_read_timeout(Some(TIMEOUT)).unwrap();
    let mut reader = BufReader::new(&reconnected);
    loop {
        if let Notice::Dictation(status) = receive::<Notice>(&mut reader).expect("helper status") {
            assert_ne!(status.phase, DictationPhase::Off);
            break;
        }
    }
    assert!(helper.0.try_wait().unwrap().is_none());
    send(
        &reconnected,
        &Command::Configure {
            recording: false,
            icon: false,
            dictation: Default::default(),
        },
    )
    .unwrap();
    exits(&mut helper);
}

#[test]
fn turning_recording_off_stops_a_helper_the_app_never_connected_to() {
    let directory = tempfile::tempdir().unwrap();
    let socket = socket(&directory);
    let mut helper = start(Path::new(HELPER), &socket);
    let (app, _) = connect(&socket);
    send(
        &app,
        &Command::Configure {
            recording: true,
            icon: true,
            dictation: Default::default(),
        },
    )
    .unwrap();
    drop(app);
    let supervisor = Supervisor::start(HELPER.into(), socket, relaunch(), |_| {});
    supervisor.configure(false, true, Default::default());
    exits(&mut helper);
    assert!(!supervisor.connected());
}

#[test]
fn with_recording_off_the_supervisor_never_starts_a_helper() {
    let directory = tempfile::tempdir().unwrap();
    let socket = socket(&directory);
    let supervisor = Supervisor::start(HELPER.into(), socket.clone(), relaunch(), |_| {});
    supervisor.configure(false, true, Default::default());
    std::thread::sleep(Duration::from_millis(300));
    assert!(!supervisor.connected());
    assert!(!socket.exists());
}

#[test]
fn one_helper_and_one_client_at_a_time() {
    let directory = tempfile::tempdir().unwrap();
    let socket = socket(&directory);
    let _helper = start(Path::new(HELPER), &socket);
    let (app, _) = connect(&socket);
    let mut second = start(Path::new(HELPER), &socket);
    exits(&mut second);
    let refused = UnixStream::connect(&socket).unwrap();
    assert_eq!(receive::<Notice>(&mut BufReader::new(&refused)), None);
    drop(app);
}

#[test]
fn the_supervisor_restarts_a_lost_helper_and_replaces_an_outdated_one() {
    let directory = tempfile::tempdir().unwrap();
    let socket = socket(&directory);
    let installed = directory.path().join("vorkspace-helper");
    std::fs::copy(HELPER, &installed).unwrap();
    // Left recording before an update replaced its binary.
    let mut outdated = start(&installed, &socket);
    let (app, _) = connect(&socket);
    send(
        &app,
        &Command::Configure {
            recording: true,
            icon: false,
            dictation: Default::default(),
        },
    )
    .unwrap();
    drop(app);
    let replacement = directory.path().join("next");
    // A real write, because copying clones the file and keeps its timestamp.
    std::fs::write(&replacement, std::fs::read(HELPER).unwrap()).unwrap();
    std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&replacement, &installed).unwrap();
    let notices = Arc::new(Mutex::new(Vec::new()));
    let seen = notices.clone();
    let supervisor = Supervisor::start(installed, socket, relaunch(), move |notice| {
        seen.lock().unwrap().push(notice)
    });
    supervisor.configure(true, false, Default::default());
    until(|| supervisor.connected());
    exits(&mut outdated);
    supervisor.send(Command::Shutdown);
    until(|| !supervisor.connected());
    until(|| supervisor.connected());
    supervisor.configure(false, false, Default::default());
    until(|| !supervisor.connected());
    // A shortcut held by another running copy is a fair report; a failed start isn't.
    let notices = notices.lock().unwrap();
    assert!(
        !notices
            .iter()
            .any(|n| matches!(n, Notice::Error(Some(error)) if error.contains("did not start"))),
        "{notices:?}"
    );
}

#[test]
fn a_clear_during_a_reconnect_reaches_the_helper() {
    let directory = tempfile::tempdir().unwrap();
    let socket = socket(&directory);
    let listener = UnixListener::bind(&socket).unwrap();
    let supervisor = Supervisor::start(HELPER.into(), socket, relaunch(), |_| {});
    supervisor.clear();
    supervisor.configure(true, false, Default::default());
    let (helper, _) = listener.accept().unwrap();
    send(&helper, &Notice::hello(Path::new(HELPER))).unwrap();
    let mut reader = BufReader::new(&helper);
    assert_eq!(
        receive(&mut reader),
        Some(Command::Configure {
            recording: true,
            icon: false,
            dictation: Default::default(),
        })
    );
    assert_eq!(receive(&mut reader), Some(relaunch()));
    assert_eq!(receive(&mut reader), Some(Command::Clear));
    supervisor.configure(false, false, Default::default());
}

#[test]
fn turning_recording_off_cancels_a_waiting_clear() {
    let directory = tempfile::tempdir().unwrap();
    let socket = socket(&directory);
    let listener = UnixListener::bind(&socket).unwrap();
    let supervisor = Supervisor::start(HELPER.into(), socket, relaunch(), |_| {});
    supervisor.clear();
    supervisor.configure(false, false, Default::default());
    supervisor.configure(true, false, Default::default());
    // The stop opens its own short connection, which hangs up without a command.
    let (helper, mut reader) = loop {
        let (helper, _) = listener.accept().unwrap();
        send(&helper, &Notice::hello(Path::new(HELPER))).unwrap();
        let mut reader = BufReader::new(helper.try_clone().unwrap());
        if receive::<Command>(&mut reader).is_some() {
            break (helper, reader);
        }
    };
    assert_eq!(receive(&mut reader), Some(relaunch()));
    until(|| supervisor.connected());
    supervisor.configure(false, false, Default::default());
    assert_eq!(
        receive(&mut reader),
        Some(Command::Configure {
            recording: false,
            icon: false,
            dictation: Default::default(),
        })
    );
    drop(helper);
}

#[test]
fn a_panel_action_survives_replacing_its_outdated_helper() {
    let directory = tempfile::tempdir().unwrap();
    let socket = socket(&directory);
    let listener = UnixListener::bind(&socket).unwrap();
    let notices = Arc::new(Mutex::new(Vec::new()));
    let seen = notices.clone();
    let supervisor = Supervisor::start(HELPER.into(), socket, relaunch(), move |notice| {
        seen.lock().unwrap().push(notice)
    });
    supervisor.configure(true, false, Default::default());
    let (helper, _) = listener.accept().unwrap();
    send(&helper, &Notice::hello(Path::new("/no/such/helper"))).unwrap();
    send(&helper, &Notice::Action(Action::Settings)).unwrap();
    assert_eq!(
        receive::<Command>(&mut BufReader::new(&helper)),
        Some(Command::Shutdown)
    );
    drop(helper);
    until(|| {
        notices
            .lock()
            .unwrap()
            .contains(&Notice::Action(Action::Settings))
    });
    drop(listener);
    supervisor.configure(false, false, Default::default());
}

#[test]
fn a_panel_quit_is_not_undone_by_the_supervisor() {
    let directory = tempfile::tempdir().unwrap();
    let socket = socket(&directory);
    let supervisor = Supervisor::start(HELPER.into(), socket.clone(), relaunch(), |_| {});
    supervisor.configure(true, false, Default::default());
    until(|| supervisor.connected());
    supervisor.configure(false, false, Default::default());
    until(|| !supervisor.connected());
    // Past the supervisor's retry delay, while the app is still shutting down.
    std::thread::sleep(Duration::from_millis(900));
    assert!(!supervisor.connected());
    assert!(UnixStream::connect(&socket).is_err());
}
