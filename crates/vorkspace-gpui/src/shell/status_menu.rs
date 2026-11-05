use super::*;

/// Background helper actions that belong to the workspace.
#[derive(Clone, Copy)]
pub(crate) enum Action {
    QuickComposer,
    NewMeeting,
    OpenMeeting,
    Settings,
    Quit,
}

impl Shell {
    pub(crate) fn status_menu_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::QuickComposer => self.toggle_quick_composer(window, cx),
            Action::NewMeeting | Action::OpenMeeting => {
                cx.activate(true);
                window.activate_window();
                vorkspace_ui::traffic_lights::show(window);
                self.open_notes(window, cx);
                if self.workspace.preferences.notes
                    && let Some(notes) = &self.notes
                {
                    notes.update(cx, |notes, cx| match action {
                        Action::NewMeeting => notes.new_recording_note(window, cx),
                        _ => notes.open_active_recording(cx),
                    });
                }
            }
            Action::Settings => {
                cx.activate(true);
                window.activate_window();
                vorkspace_ui::traffic_lights::show(window);
                self.open_settings(crate::settings::Section::General, window, cx);
            }
            // Keep the application action's unsaved-file and shutdown guards.
            Action::Quit => cx.defer(|cx| cx.dispatch_action(&crate::Quit)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn new_meeting_menu_action_routes_to_notes_without_recording(cx: &mut TestAppContext) {
        cx.update(crate::app_menu::init);
        let (main, _, _) = crate::shell::tests::window(cx);
        cx.update(|cx| cx.dispatch_action(&crate::background_helper::NewMeeting));
        cx.run_until_parked();
        main.update(cx, |shell, _, cx| {
            assert_eq!(shell.active_app_view, Some(app_views::AppView::Notes));
            assert!(shell.notes.is_some());
            assert!(!crate::meetings::capture::active(cx));
        })
        .unwrap();
    }

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
