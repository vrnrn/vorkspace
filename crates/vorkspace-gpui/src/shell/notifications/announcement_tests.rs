//! What VoiceOver hears for events that arrive between frames.
use gpui::TestAppContext;

#[gpui::test]
fn an_input_request_between_frames_is_spoken_once(cx: &mut TestAppContext) {
    let (window, _, session) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let chat = shell.sessions.get(&session).unwrap().clone();
            let title = chat.update(cx, |chat, _| {
                chat.request_approval_for_test("request", "Run tests?");
                chat.summary.title.clone()
            });
            shell.notification_session_changed(session, cx);
            shell.notification_session_changed(session, cx);
            assert_eq!(
                shell.notification_state.spoken,
                [format!("{title} needs your input: Run tests?")]
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_request_in_a_visible_conversation_never_doubles_as_a_toast(cx: &mut TestAppContext) {
    let (window, _, session) = crate::shell::tests::window(cx);
    window
        .update(cx, |shell, window, cx| {
            let chat = shell.sessions.get(&session).unwrap().clone();
            chat.update(cx, |chat, _| {
                chat.request_approval_for_test("request", "Run tests?")
            });
            // The conversation is on screen in an active window, but the
            // keyboard is elsewhere: its inline card already asks.
            shell.window_active = true;
            window.blur(cx);
            shell.sync_notices(cx);
            assert!(shell.notification_state.approvals.read(cx).is_empty());
            // Away from the window, the toast is how the request reaches you.
            shell.window_active = false;
            shell.sync_notices(cx);
            assert!(!shell.notification_state.approvals.read(cx).is_empty());
        })
        .unwrap();
}
