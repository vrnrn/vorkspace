//! The clipboard helper: a small menu bar process that keeps clipboard history
//! after the app quits. History stays in memory and dies with this process.
mod dictation;
mod dictation_panel;
mod history;
mod meeting_panel;
#[cfg(not(test))]
mod menu;
mod native;
mod peer;
mod service;
mod transcription;

use gpui::{App, Global};
use std::{
    fs::{File, OpenOptions},
    io::BufReader,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    process::Command as Process,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
#[cfg(not(test))]
use vorkspace_helper::Action;
use vorkspace_helper::{Command, Control, EXIT_WITH_CLIENT_ENV, Notice, Step, receive, send};

/// How long a helper waits for the app that started it.
const FIRST_CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

enum Event {
    Connected(UnixStream),
    Command(Command),
    Disconnected,
}

#[derive(Default)]
struct Helper {
    exit_with_client: bool,
    exit_requested: bool,
    media_requested: bool,
    background_enabled: bool,
    control: Control,
    client: Option<UnixStream>,
    #[cfg(not(test))]
    status: Option<vorkspace_macos::status_menu::Status>,
    reported_error: Option<String>,
    relaunch: Option<(String, Vec<String>)>,
}
impl Global for Helper {}

/// A lock rather than the socket decides ownership, so a crashed helper's leftover
/// socket can be replaced without racing a live one.
fn claim(socket: &Path) -> Option<(File, UnixListener)> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(socket.with_extension("lock"))
        .ok()?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return None;
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket).ok()?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).ok()?;
    Some((lock, listener))
}

/// One app at a time: a second client is closed before it hears Hello.
fn accept(listener: UnixListener, events: async_channel::Sender<Event>) {
    let occupied = Arc::new(AtomicBool::new(false));
    for stream in listener.incoming().flatten() {
        if !peer::trusted(&stream) || occupied.swap(true, Ordering::SeqCst) {
            continue;
        }
        let Ok(writer) = stream.try_clone() else {
            occupied.store(false, Ordering::SeqCst);
            continue;
        };
        let _ = events.send_blocking(Event::Connected(writer));
        let events = events.clone();
        let occupied = occupied.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            while let Some(command) = receive(&mut reader) {
                let _ = events.send_blocking(Event::Command(command));
            }
            // In this order, so a replacement's Connected always follows this.
            let _ = events.send_blocking(Event::Disconnected);
            occupied.store(false, Ordering::SeqCst);
        });
    }
}

fn run(step: Option<Step>, cx: &mut App) {
    match step {
        Some(Step::Deliver(action)) => notify(&Notice::Action(action), cx),
        Some(Step::LaunchApp) => {
            if let Some((program, arguments)) = &cx.global::<Helper>().relaunch {
                let _ = Process::new(program).args(arguments).spawn();
            }
        }
        Some(Step::Exit) => {
            if transcription::active(cx) {
                cx.global_mut::<Helper>().exit_requested = true;
                transcription::finish(cx);
            } else {
                dictation::shutdown(cx);
            }
        }
        None => {}
    }
}

fn notify(notice: &Notice, cx: &App) {
    if let Some(client) = &cx.global::<Helper>().client {
        let _ = send(client, notice);
    }
}

/// Called by the service for the panel's app buttons.
#[cfg(not(test))]
pub(crate) fn action(action: Action, cx: &mut App) {
    let step = cx.global_mut::<Helper>().control.action(action);
    run(Some(step), cx);
}

#[cfg(not(test))]
pub(crate) fn open_meeting(cx: &mut App) {
    action(Action::OpenMeeting, cx);
}

/// The app routes menu intents; the recorder revalidates the active take.
/// Clipboard presentation never owns or directly controls audio state.
#[cfg(not(test))]
fn meeting_menu_action(run: Option<String>, cx: &mut App) {
    if let Some(run) = run {
        if transcription::active_run(cx) == Some(run.as_str()) {
            transcription::finish(cx);
        }
    } else if !transcription::active(cx) {
        action(Action::NewMeeting, cx);
    }
}

fn report_error(cx: &mut App) {
    let error = service::get(cx).and_then(|service| service.read(cx).error.clone());
    let helper = cx.global_mut::<Helper>();
    if helper.reported_error != error {
        helper.reported_error = error.clone();
        notify(&Notice::Error(error), cx);
    }
}

#[cfg(not(test))]
fn sync_status(icon: bool, cx: &mut App) {
    if !icon {
        cx.global_mut::<Helper>().status = None;
    } else if cx.global::<Helper>().status.is_none() {
        let app = std::cell::RefCell::new(cx.to_async());
        let open = std::rc::Rc::new(move || {
            // AppKit must finish the status button's tracking callback before
            // the panel claims key-window ownership.
            app.borrow_mut().update(|cx| {
                cx.spawn(async move |cx| cx.update(|cx| toggle(true, cx)))
                    .detach();
            });
        });
        cx.global_mut::<Helper>().status = vorkspace_macos::status_menu::Status::new(open);
    }
}
#[cfg(test)]
fn sync_status(_: bool, _: &mut App) {}

#[cfg(not(test))]
pub(crate) fn toggle(status_click: bool, cx: &mut App) {
    let anchor = cx
        .global::<Helper>()
        .status
        .as_ref()
        .and_then(|status| status.anchor())
        .unwrap_or_else(|| {
            let screen =
                objc2_app_kit::NSScreen::mainScreen(objc2::MainThreadMarker::new().unwrap())
                    .unwrap();
            let frame = screen.visibleFrame();
            objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(
                    frame.origin.x + frame.size.width - 200.,
                    frame.origin.y + frame.size.height,
                ),
                objc2_foundation::NSSize::ZERO,
            )
        });
    if let Some(service) = service::get(cx) {
        let recording_run = transcription::active_run(cx).map(str::to_owned);
        service.update(cx, |service, cx| {
            service.toggle_menu(
                anchor,
                status_click,
                recording_run.as_deref(),
                meeting_menu_action,
                cx,
            )
        });
    }
}
#[cfg(test)]
pub(crate) fn toggle(_: bool, _: &mut App) {}

fn handle(event: Event, hello: &Notice, cx: &mut App) {
    match event {
        Event::Connected(stream) => {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
            let _ = send(&stream, hello);
            let helper = cx.global_mut::<Helper>();
            helper.client = Some(stream);
            helper.reported_error = None;
            let step = helper.control.connected();
            report_error(cx);
            dictation::status(cx);
            transcription::status(cx);
            run(step, cx);
        }
        Event::Disconnected => {
            if cx.global::<Helper>().exit_with_client {
                cx.global_mut::<Helper>().client = None;
                run(Some(Step::Exit), cx);
                return;
            }
            transcription::finish(cx);
            cx.global_mut::<Helper>().media_requested = false;
            let helper = cx.global_mut::<Helper>();
            helper.client = None;
            // A media turn retains the helper until native finalization commits.
            if transcription::active(cx) {
                cx.global_mut::<Helper>().control.configure(true);
            }
            let helper = cx.global_mut::<Helper>();
            let step = helper.control.disconnected();
            run(step, cx);
        }
        Event::Command(Command::Configure {
            recording,
            icon,
            dictation,
        }) => {
            cx.global_mut::<Helper>().background_enabled = recording || dictation.enabled;
            let keep = recording
                || dictation.enabled
                || cx.global::<Helper>().media_requested
                || transcription::active(cx);
            let step = cx.global_mut::<Helper>().control.configure(keep);
            service::configure(recording, cx);
            dictation::configure(dictation, cx);
            sync_status(icon, cx);
            report_error(cx);
            run(step, cx);
        }
        Event::Command(Command::Relaunch { program, arguments }) => {
            cx.global_mut::<Helper>().relaunch = Some((program, arguments));
        }
        Event::Command(Command::KeepAlive(wanted)) => {
            cx.global_mut::<Helper>().media_requested = wanted;
            audio_finished(cx);
        }
        Event::Command(Command::StartAudio(request)) => transcription::start(request, cx),
        Event::Command(Command::FinishAudio { id, owner, run }) => {
            transcription::finish_owned(&id, &owner, &run, cx)
        }
        Event::Command(Command::SetUpDictation) => dictation::setup(cx),
        Event::Command(Command::CancelDictation) => dictation::cancel(cx),
        Event::Command(Command::Clear) => service::clear(cx),
        Event::Command(Command::Toggle) => toggle(false, cx),
        Event::Command(Command::Shutdown) => run(Some(Step::Exit), cx),
    }
}

fn audio_finished(cx: &mut App) {
    if cx.global::<Helper>().exit_requested && !transcription::active(cx) {
        dictation::shutdown(cx);
        return;
    }
    let keep = cx.global::<Helper>().background_enabled
        || cx.global::<Helper>().media_requested
        || transcription::active(cx);
    let step = cx.global_mut::<Helper>().control.configure(keep);
    run(step, cx);
}

fn socket_argument() -> Option<PathBuf> {
    let mut arguments = std::env::args_os().skip(1);
    (arguments.next()? == "--socket")
        .then(|| arguments.next())
        .flatten()
        .map(PathBuf::from)
}

fn main() {
    let Some(socket) = socket_argument() else {
        eprintln!("vorkspace-helper is started by vorkspace: --socket PATH");
        std::process::exit(2);
    };
    let Some((_lock, listener)) = claim(&socket) else {
        return;
    };
    // Read once: after an update, the file on disk is no longer what's running.
    let hello = Notice::hello(&std::env::current_exe().unwrap_or_default());
    let (events, received) = async_channel::unbounded();
    std::thread::spawn(move || accept(listener, events));
    let exit_with_client = std::env::var(EXIT_WITH_CLIENT_ENV).as_deref() == Ok("1");
    gpui::Application::with_platform(std::rc::Rc::new(gpui_macos::MacPlatform::new(false)))
        // No Dock icon or menu bar, even for an unbundled build without LSUIElement.
        .with_windowing(gpui::WindowingRequest::Headless)
        .run(move |cx: &mut App| {
            cx.set_global(Helper {
                exit_with_client,
                ..Helper::default()
            });
            dictation::init(cx);
            transcription::init(cx);
            service::configure(false, cx);
            if let Some(service) = service::get(cx) {
                cx.observe(&service, |_, cx| report_error(cx)).detach();
            }
            cx.spawn(async move |cx| {
                while let Ok(event) = received.recv().await {
                    cx.update(|cx| handle(event, &hello, cx));
                }
            })
            .detach();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(FIRST_CLIENT_TIMEOUT).await;
                cx.update(|cx| {
                    if cx.global::<Helper>().control.abandoned() {
                        cx.quit();
                    }
                });
            })
            .detach();
        });
}
