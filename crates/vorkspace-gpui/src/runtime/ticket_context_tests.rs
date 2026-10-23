use super::*;
#[test]
fn inaccessible_ticket_admission_keeps_queued_user_identity_and_never_dispatches() {
    for cancelled in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let db = Store::open_vorkspace(&directory.path().join("sessions.db")).unwrap();
        let summary = db.create("codex", "/missing/ticket-project").unwrap();
        let text = "Work on vorkspace://context/jira?url=https%3A%2F%2Fteam.atlassian.net%2Fbrowse%2FDEMO-42";
        db.save_follow_ups(
            summary.id,
            &vorkspace_core::follow_up::State {
                revision: 1,
                messages: vec![vorkspace_core::follow_up::Message {
                    id: "queued-ticket".into(),
                    text: text.into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .unwrap();
        let store = Arc::new(Mutex::new(db));
        let (tx, rx) = async_channel::bounded(8);
        let (_sender, controls) = std::sync::mpsc::channel();
        run_turn_using(
            TurnInput {
                summary: summary.clone(),
                prompt: text.into(),
                settings: Settings {
                    bot_message_id: Some("queued-ticket".into()),
                    ..Default::default()
                },
            },
            controls,
            store.clone(),
            Arc::new(AtomicBool::new(cancelled)),
            tx,
            |_| panic!("Ticket admission must finish before resolving or launching a provider"),
        );
        let update = rx.try_recv().unwrap();
        assert!(update.finished);
        if !cancelled {
            assert!(
                update.status.contains("Connect this Jira site"),
                "{}",
                update.status
            );
        }
        let store = locked(&store).unwrap();
        assert!(store.local_submissions(summary.id).unwrap().is_empty());
        let queued = store.follow_ups(summary.id).unwrap();
        assert_eq!(queued.messages[0].id, "queued-ticket");
        assert_eq!(queued.messages[0].text, text);
    }
}
