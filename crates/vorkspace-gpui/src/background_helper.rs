//! Connection and process lifetime for the Vorkspace background helper.
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global};
#[cfg(not(test))]
use vorkspace_helper::Supervisor;
use vorkspace_helper::{
    Action, AudioNotice, Command, DictationConfig, DictationPhase, DictationStatus, Notice,
};
// Keep the existing action identifiers for keybindings.
gpui::actions!(clipboard, [Toggle, Settings, Composer, Quit, Reopen]);

struct BackgroundHelper(Entity<Service>);
impl Global for BackgroundHelper {}

pub(crate) enum CaptureEvent {
    Connected,
    Audio(AudioNotice),
}

impl EventEmitter<CaptureEvent> for Service {}
#[derive(Default)]
pub(crate) struct Service {
    pub error: Option<String>,
    capture_active: bool,
    icon: bool,
    recording: bool,
    dictation: DictationConfig,
    pub dictation_status: DictationStatus,
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
    cx.try_global::<BackgroundHelper>().map(|c| c.0.clone())
}
fn service(cx: &mut App) -> Entity<Service> {
    get(cx).unwrap_or_else(|| {
        let service = cx.new(|cx| {
            let mut service = Service::default();
            service.start(cx);
            service
        });
        cx.set_global(BackgroundHelper(service.clone()));
        service
    })
}
pub(crate) fn configure(recording: bool, icon: bool, dictation: DictationConfig, cx: &mut App) {
    service(cx).update(cx, |s, cx| {
        s.icon = icon;
        s.recording = recording;
        s.dictation = dictation.clone();
        if !dictation.enabled {
            s.dictation_status = DictationStatus::default();
        }
        #[cfg(not(test))]
        {
            if let Some(supervisor) = &s.supervisor {
                supervisor.configure(recording, icon, dictation.clone());
            }
            s.sync_status(cx);
        }
        cx.notify();
    });
}
impl Service {
    #[cfg(not(test))]
    fn start(&mut self, cx: &mut Context<Self>) {
        let Ok(app) = std::env::current_exe() else {
            self.error = Some("Could not find the Vorkspace helper.".into());
            return;
        };
        let helper = vorkspace_helper::helper_executable(&app);
        let socket = vorkspace_helper::socket_path(&helper);
        let (notices, received) = async_channel::bounded(256);
        let arguments = std::env::args_os()
            .skip(1)
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let relaunch = vorkspace_helper::relaunch_command(&app, arguments);
        self.supervisor = Some(Supervisor::start(helper, socket, relaunch, move |notice| {
            let _ = notices.send_blocking(notice);
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
        if !self.icon || self.recording || self.dictation.enabled || self.capture_active {
            self.status = None;
        } else if self.status.is_none() {
            let app = std::cell::RefCell::new(cx.to_async());
            self.status = vorkspace_macos::status_menu::Status::new(std::rc::Rc::new(move || {
                app.borrow_mut().update(|cx| cx.dispatch_action(&Reopen));
            }));
        }
    }

    fn clear(&self) {
        #[cfg(not(test))]
        if let Some(supervisor) = &self.supervisor {
            supervisor.clear();
        }
    }

    pub(crate) fn send(&self, command: Command) -> bool {
        #[cfg(not(test))]
        if let Some(supervisor) = &self.supervisor {
            return supervisor.send(command);
        }
        let _ = command;
        false
    }

    pub(crate) fn connected(&self) -> bool {
        #[cfg(not(test))]
        return self.supervisor.as_ref().is_some_and(Supervisor::connected);
        #[cfg(test)]
        false
    }

    pub(crate) fn dictation_config(&self) -> &DictationConfig {
        &self.dictation
    }

    /// The capture owner supplies the desired lifetime. The helper owns the process.
    pub(crate) fn set_capture_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.capture_active == active {
            return;
        }
        self.capture_active = active;
        #[cfg(not(test))]
        {
            if let Some(supervisor) = &self.supervisor {
                supervisor.keep_alive(active);
            }
            self.sync_status(cx);
        }
        cx.notify();
    }

    fn notice(&mut self, notice: Notice, cx: &mut Context<Self>) {
        match notice {
            Notice::Error(error) => {
                if error.is_none() {
                    cx.emit(CaptureEvent::Connected);
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
            Notice::Dictation(status) => {
                if self.dictation.enabled || status.phase == DictationPhase::Off {
                    self.dictation_status = status;
                    cx.notify();
                }
            }
            Notice::Audio(notice) => cx.emit(CaptureEvent::Audio(notice)),
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
            && if s.recording || s.dictation.enabled || s.capture_active {
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
        if s.stop_helper_on_quit {
            // Through the desired state, so that the supervisor can't start a
            // replacement during the app's own slower shutdown.
            #[cfg(not(test))]
            if let Some(supervisor) = &s.supervisor {
                supervisor.configure(false, false, DictationConfig::default());
            }
        }
    }
}

/// Build the helper's complete, replayable preference snapshot.
pub(crate) fn dictation_configuration(
    prefs: &vorkspace_ui::preferences::Preferences,
) -> DictationConfig {
    DictationConfig {
        enabled: prefs.dictation_enabled,
        microphone: prefs.dictation_microphone.clone(),
        words: prefs.dictation_words.clone(),
        shortcut: match prefs.keybindings.get("App: Dictation") {
            Some(shortcut) => shortcut.clone().unwrap_or_default(),
            None => DictationConfig::default().shortcut,
        },
    }
}
pub(crate) fn ensure(cx: &mut App) -> Entity<Service> {
    service(cx)
}
pub(crate) fn dictation_setup(cx: &mut App) {
    service(cx).update(cx, |s, cx| {
        if !s.send(Command::SetUpDictation) {
            s.dictation_status = DictationStatus {
                phase: DictationPhase::Error,
                message: "The background helper is connecting. Try again in a moment.".into(),
                active: false,
            };
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
            configure(true, true, DictationConfig::default(), cx);
            get(cx).unwrap()
        });
        service.update(cx, |s, cx| s.notice(Notice::Action(Action::Quit), cx));
        assert!(service.read_with(cx, |s, _| s.stop_helper_on_quit));
        cx.update(quit_cancelled);
        assert!(!service.read_with(cx, |s, _| s.stop_helper_on_quit));
    }

    #[gpui::test]
    fn close_to_tray_needs_the_icon(cx: &mut TestAppContext) {
        cx.update(|cx| configure(true, false, DictationConfig::default(), cx));
        assert!(!cx.update(|cx| menu_bar_ready(cx)));
        cx.update(|cx| configure(false, true, DictationConfig::default(), cx));
        assert!(cx.update(|cx| menu_bar_ready(cx)));
    }
}
