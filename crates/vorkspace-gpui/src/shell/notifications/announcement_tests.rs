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
