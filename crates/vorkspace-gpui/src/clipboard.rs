//! Keeps the clipboard helper in step with preferences. History, ⌘⇧C and the menu
//! bar icon live in the helper so that they outlive this app. With recording off
//! there is no helper, so this app shows the icon that Close to Tray depends on.
use gpui::{App, AppContext, Context, Entity, Global};
#[cfg(not(test))]
use vorkspace_clipboard::Supervisor;
use vorkspace_clipboard::{Action, Command, ModelState, Notice};
gpui::actions!(clipboard, [Toggle, Settings, Composer, Quit, Reopen]);

struct Clipboard(Entity<Service>);
impl Global for Clipboard {}
#[derive(Default)]
pub(crate) struct Service {
    pub error: Option<String>,
    icon: bool,
    recording: bool,
    dictation: bool,
    model_request: Option<(u64, bool)>,
    pending_model: Option<Command>,
    pub model_notice: Option<(u64, ModelState)>,
    pub dictation_status: String,
    pub dictation_ready: bool,
    /// The panel's Quit also stops the helper, once the app really does quit.
    stop_helper_on_quit: bool,
    #[cfg(not(test))]
    supervisor: Option<Supervisor>,
    #[cfg(not(test))]
    status: Option<vorkspace_macos::status_menu::Status>,
    #[cfg(not(test))]
    _notices: Option<gpui::Task<()>>,
}
pub(crate) fn get(cx: &App) -> Option<Entity<Service>> {
    cx.try_global::<Clipboard>().map(|c| c.0.clone())
}
fn service(cx: &mut App) -> Entity<Service> {
    get(cx).unwrap_or_else(|| {
        let service = cx.new(|cx| {
            let mut service = Service::default();
            service.start(cx);
            service
        });
        cx.set_global(Clipboard(service.clone()));
        service
    })
}
pub(crate) fn configure(recording: bool, icon: bool, dictation: bool, cx: &mut App) {
    service(cx).update(cx, |s, cx| {
        s.icon = icon;
        s.recording = recording;
        s.dictation = dictation;
        if dictation
            && s.model_notice
                .as_ref()
                .is_some_and(|(_, state)| *state == ModelState::Ready)
        {
            s.model_request = None;
        }
        #[cfg(not(test))]
        {
            if let Some(supervisor) = &s.supervisor {
                supervisor.configure(recording, icon, dictation, s.model_request.is_some());
            }
            s.sync_status(cx);
        }
        cx.notify();
    });
}
impl Service {
    #[cfg(test)]
    pub(crate) fn test_model_reply(
        &mut self,
        request: u64,
        state: ModelState,
        cx: &mut Context<Self>,
    ) {
        self.notice(Notice::Model { request, state }, cx);
    }

    #[cfg(not(test))]
    fn start(&mut self, cx: &mut Context<Self>) {
        let Ok(app) = std::env::current_exe() else {
            self.error = Some("Could not find the clipboard helper.".into());
            return;
        };
        let helper = vorkspace_clipboard::helper_executable(&app);
        let socket = vorkspace_clipboard::socket_path(&helper);
        let (notices, received) = async_channel::unbounded();
        let arguments = std::env::args_os()
            .skip(1)
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let relaunch = vorkspace_clipboard::relaunch_command(&app, arguments);
        self.supervisor = Some(Supervisor::start(helper, socket, relaunch, move |notice| {
            let _ = notices.try_send(notice);
        }));
        self._notices = Some(cx.spawn(async move |this, cx| {
            while let Ok(notice) = received.recv().await {
                let _ = this.update(cx, |this, cx| this.notice(notice, cx));
            }
        }));
    }
    #[cfg(test)]
    fn start(&mut self, _: &mut Context<Self>) {}

    #[cfg(not(test))]
    fn sync_status(&mut self, cx: &mut Context<Self>) {
        if !self.icon || self.recording || self.dictation || self.model_request.is_some() {
            self.status = None;
        } else if self.status.is_none() {
            let app = std::cell::RefCell::new(cx.to_async());
            self.status = vorkspace_macos::status_menu::Status::new(std::rc::Rc::new(move || {
                app.borrow_mut().update(|cx| cx.dispatch_action(&Reopen));
            }));
        }
    }

    fn sync_configuration(&mut self, cx: &mut Context<Self>) {
        #[cfg(not(test))]
        {
            if let Some(supervisor) = &self.supervisor {
                supervisor.configure(
                    self.recording,
                    self.icon,
                    self.dictation,
                    self.model_request.is_some(),
                );
            }
            self.sync_status(cx);
        }
        let _ = cx;
    }
    fn clear(&self) {
        #[cfg(not(test))]
        if let Some(supervisor) = &self.supervisor {
            supervisor.clear();
        }
    }

    fn send(&self, command: Command) -> bool {
        #[cfg(not(test))]
        if let Some(supervisor) = &self.supervisor {
            return supervisor.send(command);
        }
        let _ = command;
        false
    }

    fn notice(&mut self, notice: Notice, cx: &mut Context<Self>) {
        match notice {
            Notice::Error(error) => {
                if error.is_none() {
                    if let Some(command) = self.pending_model.take() {
                        if !self.send(command.clone()) {
                            self.pending_model = Some(command);
                        }
                    } else if let Some((request, _)) = self.model_request {
                        self.send(Command::ModelStatus { request });
                    }
                }
                self.error = error;
                cx.notify();
            }
            Notice::Action(Action::Settings) => cx.dispatch_action(&Settings),
            Notice::Action(Action::Composer) => cx.dispatch_action(&Composer),
            Notice::Action(Action::Quit) => {
                self.stop_helper_on_quit = true;
                cx.dispatch_action(&Quit);
            }
            Notice::Dictation { status, ready } => {
                self.dictation_status = status;
                self.dictation_ready = ready;
                cx.notify();
            }
            Notice::Model { request, mut state } => {
                if let Some((current, cancelled)) = self.model_request
                    && current == request
                {
                    if cancelled && !state.terminal() && state != ModelState::Cancelling {
                        return;
                    }
                    if cancelled && state.terminal() {
                        state = ModelState::Cancelled;
                    }
                    if state.terminal() && state != ModelState::Ready {
                        self.model_request = None;
                        self.pending_model = None;
                        self.sync_configuration(cx);
                    }
                    self.dictation_status = match &state {
                        ModelState::Checking => {
                            "Checking model or connecting to background helper…".into()
                        }
                        ModelState::Downloading(bytes)
                            if *bytes == vorkspace_clipboard::dictation_model::BYTES =>
                        {
                            "Verifying English model…".into()
                        }
                        ModelState::Downloading(bytes) => format!(
                            "Downloading English model… {}%",
                            bytes * 100 / vorkspace_clipboard::dictation_model::BYTES
                        ),
                        ModelState::Cancelling => "Cancelling model download…".into(),
                        ModelState::Cancelled => "Model download cancelled.".into(),
                        ModelState::Ready => "English model verified.".into(),
                        ModelState::Missing => {
                            "Download the English model to enable dictation.".into()
                        }
                        ModelState::Error(e) => e.clone(),
                    };
                    self.model_notice = Some((request, state));
                    cx.notify();
                }
            }
            Notice::Hello { .. } => {}
        }
    }
}
pub(crate) fn clear(cx: &mut App) {
    if let Some(s) = get(cx) {
        s.read(cx).clear();
    }
}
pub(crate) fn toggle(cx: &mut App) {
    if let Some(s) = get(cx) {
        s.read(cx).send(Command::Toggle);
    }
}
/// Close to Tray needs a menu bar icon that can bring the window back.
pub(crate) fn menu_bar_ready(cx: &App) -> bool {
    get(cx).is_some_and(|s| {
        let s = s.read(cx);
        #[cfg(not(test))]
        return s.icon
            && if s.recording || s.dictation || s.model_request.is_some() {
                s.supervisor.as_ref().is_some_and(Supervisor::connected)
            } else {
                s.status.is_some()
            };
        #[cfg(test)]
        s.icon
    })
}
/// The user cancelled the quit that the panel started.
pub(crate) fn quit_cancelled(cx: &mut App) {
    if let Some(s) = get(cx) {
        s.update(cx, |s, _| s.stop_helper_on_quit = false);
    }
}
/// Only the panel's Quit ends the helper. Any other quit leaves it recording.
pub(crate) fn app_quitting(cx: &mut App) {
    if let Some(s) = get(cx) {
        let s = s.read(cx);
        if let Some((request, _)) = s.model_request {
            s.send(Command::CancelModel { request });
        }
        if s.model_request.is_some() {
            #[cfg(not(test))]
            if let Some(supervisor) = &s.supervisor {
                supervisor.configure(s.recording, s.icon, s.dictation, false);
            }
        }
        if s.stop_helper_on_quit {
            // Through the desired state, so that the supervisor can't start a
            // replacement during the app's own slower shutdown.
            #[cfg(not(test))]
            if let Some(supervisor) = &s.supervisor {
                supervisor.configure(false, false, false, false);
            }
        }
    }
}

pub(crate) fn next_model_request() -> u64 {
    static NEXT: std::sync::OnceLock<std::sync::atomic::AtomicU64> = std::sync::OnceLock::new();
    NEXT.get_or_init(|| {
        std::sync::atomic::AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
        )
    })
    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
pub(crate) fn dictation_download(request: u64, cx: &mut App) -> Result<(), String> {
    service(cx).update(cx, |s, cx| {
        if s.model_request.is_some_and(|(id, _)| id != request) {
            return Err("A previous model request is still finishing. Retry in a moment.".into());
        }
        s.model_request = Some((request, false));
        s.model_notice = Some((request, ModelState::Checking));
        s.sync_configuration(cx);
        let command = Command::DownloadModel { request };
        if !s.send(command.clone()) {
            s.pending_model = Some(command);
            s.dictation_status = "Connecting to background helper…".into();
        }
        cx.notify();
        Ok(())
    })
}
pub(crate) fn cancel_model(request: u64, cx: &mut App) {
    if let Some(service) = get(cx) {
        service.update(cx, |s, cx| {
            if s.model_request.is_some_and(|(id, _)| id == request) {
                if matches!(s.pending_model,Some(Command::DownloadModel{request:id}) if id==request)
                {
                    // Never admitted to a helper: revoke the queued action locally.
                    s.pending_model = None;
                    s.model_request = None;
                    s.model_notice = Some((request, ModelState::Cancelled));
                    s.sync_configuration(cx);
                    cx.notify();
                    return;
                }
                s.model_request = Some((request, true));
                s.model_notice = Some((request, ModelState::Cancelling));
                let command = Command::CancelModel { request };
                if !s.send(command.clone()) {
                    s.pending_model = Some(command);
                }
                cx.notify();
            }
        });
    }
}
pub(crate) fn finish_model(request: u64, cx: &mut App) {
    if let Some(service) = get(cx) {
        service.update(cx, |s, cx| {
            if s.model_request.is_some_and(|(id, _)| id == request) {
                s.model_request = None;
                s.pending_model = None;
                s.sync_configuration(cx);
            }
        });
    }
}
pub(crate) fn ensure(cx: &mut App) -> Entity<Service> {
    service(cx)
}
pub(crate) fn dictation_permissions(cx: &mut App) {
    dictation_command(Command::DictationPermissions, cx);
}
pub(crate) fn dictation_cancel(cx: &mut App) {
    dictation_command(Command::CancelDictation, cx);
}

fn dictation_command(command: Command, cx: &mut App) {
    service(cx).update(cx, |s, cx| {
        if !s.send(command) {
            s.dictation_status =
                "Background helper is reconnecting. Retry this action in a moment.".into();
            cx.notify();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn a_cancelled_panel_quit_leaves_the_helper_running(cx: &mut TestAppContext) {
        let service = cx.update(|cx| {
            configure(true, true, false, cx);
            get(cx).unwrap()
        });
        service.update(cx, |s, cx| s.notice(Notice::Action(Action::Quit), cx));
        assert!(service.read_with(cx, |s, _| s.stop_helper_on_quit));
        cx.update(quit_cancelled);
        assert!(!service.read_with(cx, |s, _| s.stop_helper_on_quit));
    }

    #[gpui::test]
    fn close_to_tray_needs_the_icon(cx: &mut TestAppContext) {
        cx.update(|cx| configure(true, false, false, cx));
        assert!(!cx.update(|cx| menu_bar_ready(cx)));
        cx.update(|cx| configure(false, true, false, cx));
        assert!(cx.update(|cx| menu_bar_ready(cx)));
    }
}

#[cfg(test)]
mod model_tests {
    use super::*;
    #[gpui::test]
    fn unadmitted_download_is_retained_for_reconnect_and_cancel_revokes_it(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            assert!(dictation_download(10, cx).is_ok());
            let service = get(cx).unwrap();
            service.update(cx, |s, cx| {
                assert_eq!(
                    s.pending_model,
                    Some(Command::DownloadModel { request: 10 })
                );
                assert_eq!(s.model_request, Some((10, false)));
                s.notice(Notice::Error(None), cx);
                assert_eq!(
                    s.pending_model,
                    Some(Command::DownloadModel { request: 10 })
                );
            });
            cancel_model(10, cx);
            assert!(service.read(cx).model_request.is_none());
            assert!(service.read(cx).pending_model.is_none());
        });
    }
    #[gpui::test]
    fn admitted_cancellation_retains_setup_until_join_and_fences_old_ready(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            dictation_download(10, cx).unwrap();
            let service = get(cx).unwrap();
            service.update(cx, |s, _| s.pending_model = None);
            cancel_model(10, cx);
            assert_eq!(service.read(cx).model_request, Some((10, true)));
            assert!(dictation_download(11, cx).is_err());
            service.update(cx, |s, cx| {
                s.notice(
                    Notice::Model {
                        request: 10,
                        state: ModelState::Downloading(2048),
                    },
                    cx,
                )
            });
            assert_eq!(
                service.read(cx).model_notice,
                Some((10, ModelState::Cancelling))
            );
            service.update(cx, |s, cx| {
                s.notice(
                    Notice::Model {
                        request: 10,
                        state: ModelState::Ready,
                    },
                    cx,
                )
            });
            assert!(service.read(cx).model_request.is_none());
            assert_eq!(
                service.read(cx).model_notice,
                Some((10, ModelState::Cancelled))
            );
            dictation_download(11, cx).unwrap();
            service.update(cx, |s, cx| {
                s.notice(
                    Notice::Model {
                        request: 10,
                        state: ModelState::Downloading(100),
                    },
                    cx,
                )
            });
            assert_eq!(service.read(cx).model_request, Some((11, false)));
            assert_eq!(
                service.read(cx).model_notice,
                Some((11, ModelState::Checking))
            );
        });
    }
}
