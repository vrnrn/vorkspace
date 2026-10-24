use super::*;
use crate::status_menu::Action;

impl Shell {
    #[cfg(all(target_os = "macos", not(test)))]
    pub(super) fn sync_status_menu(&mut self, window: &Window, cx: &mut Context<Self>) {
        // Retain the saved preference key so existing users keep their choice.
        if !self.workspace.preferences.bot_menu_bar_icon {
            self.status_menu = None;
            return;
        }
        if self.status_menu.is_some() {
            return;
        }
        let owner = cx.weak_entity();
        let target = window.window_handle();
        let app = std::cell::RefCell::new(cx.to_async());
        let run = std::rc::Rc::new(move |action| {
            let owner = owner.clone();
            app.borrow_mut().update(|cx| {
                // AppKit must finish the status button's tracking callback
                // before the popup claims key-window ownership.
                cx.spawn(async move |cx| {
                    let _ = target.update(cx, |_, window, cx| {
                        let _ = owner
                            .update(cx, |shell, cx| shell.status_menu_action(action, window, cx));
                    });
                })
                .detach();
            });
        });
        self.status_menu = crate::status_menu::Status::new(run);
    }

    pub(crate) fn status_menu_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::Clipboard => self.toggle_clipboard(window, cx),
            Action::QuickComposer => self.toggle_quick_composer(window, cx),
            Action::Settings => {
                cx.activate(true);
                window.activate_window();
                #[cfg(target_os = "macos")]
                crate::traffic_lights::show(window);
                self.open_settings(crate::settings::Section::General, window, cx);
            }
            // Keep the application action's unsaved-file and shutdown guards.
            Action::Quit => cx.defer(|cx| cx.dispatch_action(&crate::Quit)),
        }
    }
    pub(crate) fn toggle_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(all(target_os = "macos", not(test)))]
        if let Some(service) = crate::clipboard::get(cx) {
            let anchor = self
                .status_menu
                .as_ref()
                .and_then(|status| status.anchor())
                .unwrap_or_else(|| {
                    let screen = objc2_app_kit::NSScreen::mainScreen(
                        objc2::MainThreadMarker::new().unwrap(),
                    )
                    .unwrap();
                    let frame = screen.visibleFrame();
                    (
                        frame.origin.x + frame.size.width - 200.,
                        frame.origin.y + frame.size.height,
                    )
                });
            service.update(cx, |service, cx| {
                service.toggle_menu(anchor.0, anchor.1, cx)
            });
        }
        let _ = (window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn menu_composer_opens_without_creating_a_session_or_selecting_a_bot(cx: &mut TestAppContext) {
        let (main, _, _) = crate::shell::tests::window(cx);
        let (order, active) = main
            .update(cx, |shell, window, cx| {
                shell.workspace.preferences.bots = false;
                shell.apply_preferences(cx);
                let active = shell
                    .bots
                    .as_ref()
                    .and_then(|bots| bots.read(cx).active.clone());
                let order = shell.order.clone();
                shell.status_menu_action(Action::QuickComposer, window, cx);
                assert!(shell.quick.window.is_some());
                (order, active)
            })
            .unwrap();
        cx.run_until_parked();
        main.update(cx, |shell, window, cx| {
            assert_eq!(shell.order, order);
            assert_eq!(
                shell
                    .bots
                    .as_ref()
                    .and_then(|bots| bots.read(cx).active.clone()),
                active
            );
            shell.status_menu_action(Action::QuickComposer, window, cx);
            assert!(shell.quick.window.is_none());
        })
        .unwrap();
    }
}
