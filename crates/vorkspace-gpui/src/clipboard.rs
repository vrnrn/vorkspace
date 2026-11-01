//! Keeps the clipboard helper in step with preferences. History, ⌘⇧C and the menu
//! bar icon live in the helper so that they outlive this app. With recording off
//! there is no helper, so this app shows the icon that Close to Tray depends on.
use gpui::{App, AppContext, Context, Entity, Global};
#[cfg(not(test))]
use vorkspace_clipboard::Supervisor;
use vorkspace_clipboard::{Action, Command, Notice};
gpui::actions!(clipboard, [Toggle, Settings, Composer, Quit, Reopen]);

struct Clipboard(Entity<Service>);
impl Global for Clipboard {}
#[derive(Default)]
pub(crate) struct Service {
    pub error: Option<String>,
    icon: bool,
    recording: bool,
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
pub(crate) fn configure(recording: bool, icon: bool, cx: &mut App) {
    service(cx).update(cx, |s, cx| {
        s.icon = icon;
        s.recording = recording;
        #[cfg(not(test))]
        {
            if let Some(supervisor) = &s.supervisor {
                supervisor.configure(recording, icon);
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
            self.error = Some("Could not find the clipboard helper.".into());
            return;
        };
        let helper = vorkspace_clipboard::helper_executable(&app);
        let socket = vorkspace_clipboard::socket_path(&helper);
        let (notices, received) = async_channel::unbounded();
        self.supervisor = Some(Supervisor::start(helper, socket, move |notice| {
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
        if !self.icon || self.recording {
            self.status = None;
        } else if self.status.is_none() {
            let app = std::cell::RefCell::new(cx.to_async());
            self.status = vorkspace_macos::status_menu::Status::new(std::rc::Rc::new(move || {
                app.borrow_mut().update(|cx| cx.dispatch_action(&Reopen));
            }));
        }
    }

    fn send(&self, command: Command) {
        #[cfg(not(test))]
        if let Some(supervisor) = &self.supervisor {
            supervisor.send(command);
        }
        let _ = command;
    }

    fn notice(&mut self, notice: Notice, cx: &mut Context<Self>) {
        match notice {
            Notice::Error(error) => {
                self.error = error;
                cx.notify();
            }
            Notice::Action(Action::Settings) => cx.dispatch_action(&Settings),
            Notice::Action(Action::Composer) => cx.dispatch_action(&Composer),
            Notice::Action(Action::Quit) => {
                self.stop_helper_on_quit = true;
                cx.dispatch_action(&Quit);
            }
            Notice::Hello { .. } => {}
        }
    }
}
pub(crate) fn clear(cx: &mut App) {
    if let Some(s) = get(cx) {
        s.read(cx).send(Command::Clear);
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
            && if s.recording {
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
            s.send(Command::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn a_cancelled_panel_quit_leaves_the_helper_running(cx: &mut TestAppContext) {
        let service = cx.update(|cx| {
            configure(true, true, cx);
            get(cx).unwrap()
        });
        service.update(cx, |s, cx| s.notice(Notice::Action(Action::Quit), cx));
        assert!(service.read_with(cx, |s, _| s.stop_helper_on_quit));
        cx.update(quit_cancelled);
        assert!(!service.read_with(cx, |s, _| s.stop_helper_on_quit));
    }

    #[gpui::test]
    fn close_to_tray_needs_the_icon(cx: &mut TestAppContext) {
        cx.update(|cx| configure(true, false, cx));
        assert!(!cx.update(|cx| menu_bar_ready(cx)));
        cx.update(|cx| configure(false, true, cx));
        assert!(cx.update(|cx| menu_bar_ready(cx)));
    }
}
