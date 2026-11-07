use super::*;
use gpui::{TestAppContext, WindowHandle};
use vorkspace_core::{store::Store, transcript::Row};

fn update(index: usize, finished: bool) -> vorkspace_runtime::runtime::Update {
    vorkspace_runtime::runtime::Update {
        source_len: None,
        bot_turn_start: None,
        activity: None,
        worktree_cwd: None,
        commands: None,
        provider_turn_id: None,
        turn_ready: false,
        follow_up_delivered: vec![],
        follow_up_errors: vec![],
        delivered: None,
        start: index,
        rows: vec![Row {
            role: "assistant".into(),
            text: format!("Synthetic update {index}"),
            first: true,
            sent_at: None,
        }],
        provider_id: None,
        provider_reset: false,
        model: None,
        config_model: None,
        config_settings: None,
        context: None,
        metrics: None,
        usage_limit: None,
        approvals: vec![],
        approvals_resolved: vec![],
        questions: vec![],
        questions_resolved: vec![],
        handoff: None,
        status: format!("Synthetic status {index}"),
        finished,
        history_recovery: None,
    }
}

fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let summary = store.create("codex", "/missing/receiver-fixture").unwrap();
    let window = cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view =
                    Conversation::new(summary, Arc::new(std::sync::Mutex::new(store)), cx);
                view.loaded = true;
                view
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
}

fn start_receiver(
    window: WindowHandle<Conversation>,
    rx: async_channel::Receiver<vorkspace_runtime::runtime::Update>,
    cx: &mut TestAppContext,
) {
    window
        .update(cx, |view, _, cx| {
            view.begin_turn(cx);
            view.receive_turn_updates(
                rx,
                view.submission_generation,
                view.summary.provider.clone(),
                view.settings.account.id.clone(),
                None,
                cx,
            );
        })
        .unwrap();
}

/// Tick runnable tasks without advancing timers. Once the receiver starts,
/// inspect what a single foreground poll did before any continuation can run.
fn first_poll(window: WindowHandle<Conversation>, cx: &mut TestAppContext) {
    for _ in 0..1000 {
        if window
            .read_with(cx, |view, _| view.status.starts_with("Synthetic status"))
            .unwrap()
        {
            return;
        }
        assert!(cx.executor().tick(), "receiver must be runnable");
    }
    panic!("receiver was never polled");
}

pub(super) fn settle_turn(window: WindowHandle<Conversation>, cx: &mut TestAppContext) {
    let entity = window.entity(cx).unwrap();
    // run_until_parked drains runnable work, but does not advance this
    // scheduler's clock. Await the semantic transition, driving its timers.
    let executor = cx.foreground_executor().clone();
    executor.block_test(cx.condition(&entity, |view, _| !view.busy && !view.stopping));
    cx.run_until_parked();
}

#[gpui::test]
fn ready_turn_updates_return_to_the_executor_before_draining_the_queue(cx: &mut TestAppContext) {
    let window = conversation(cx);
    // Match the production channel capacity. recv().await is immediately ready
    // for every entry here; a bounded channel alone is not a scheduling bound.
    let (tx, rx) = async_channel::bounded(8);
    for index in 0..8 {
        tx.try_send(update(index, index == 7)).unwrap();
    }
    start_receiver(window, rx, cx);
    first_poll(window, cx);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.status, "Synthetic status 0");
            assert_eq!(
                view.transcript.read(cx).source_rows(),
                update(0, false).rows
            );
            assert!(view.busy);
            // Control/user input gets an opportunity between atomic patches.
            view.input
                .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
        })
        .unwrap();
    drop(tx);
    settle_turn(window, cx);
    window
        .update(cx, |view, _, cx| {
            let expected: Vec<_> = (0..8).flat_map(|index| update(index, false).rows).collect();
            assert_eq!(view.transcript.read(cx).source_rows(), expected);
            assert_eq!(view.status, "Synthetic status 7");
            assert!(!view.busy && !view.stopping);
            assert_eq!(view.input.read(cx).content.as_str(), "Unsent draft");
        })
        .unwrap();
}

#[gpui::test]
fn superseding_a_yielded_receiver_protects_the_successor(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let (tx, rx) = async_channel::bounded(8);
    let pending = rx.clone();
    for index in 0..8 {
        tx.try_send(update(index, index == 7)).unwrap();
    }
    start_receiver(window, rx, cx);
    first_poll(window, cx);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.status, "Synthetic status 0");
            view.begin_turn(cx);
            view.status = "Successor".into();
        })
        .unwrap();
    drop(tx);
    // The successor deliberately remains busy, so drain the known old burst
    // by advancing each continuation rather than waiting for idle state.
    for _ in 0..8 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1));
        cx.run_until_parked();
    }
    assert!(pending.is_empty());
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.status, "Successor");
            assert!(view.busy && !view.stopping);
            assert_eq!(
                view.transcript.read(cx).source_rows(),
                update(0, false).rows
            );
        })
        .unwrap();
}

#[gpui::test]
fn stop_between_updates_skips_streaming_but_accepts_the_final_replay(cx: &mut TestAppContext) {
    let window = conversation(cx);
    window
        .update(cx, |view, _, _| {
            view.summary.provider_id = Some("old-provider-session".into());
            view.context = Some(vorkspace_core::context::Usage {
                used: 95_000,
                window: Some(100_000),
            });
        })
        .unwrap();
    let (tx, rx) = async_channel::bounded(8);
    tx.try_send(update(0, false)).unwrap();
    let mut skipped = update(1, false);
    skipped.provider_reset = true;
    skipped.context = Some(Default::default());
    skipped.approvals.push(Approval {
        id: "late-approval".into(),
        title: "Synthetic approval".into(),
        detail: String::new(),
        can_remember: false,
    });
    tx.try_send(skipped).unwrap();
    // The worker replays the whole accepted turn on cancellation, rather than
    // depending on intermediate patches which the stopping UI must ignore.
    let mut final_update = update(0, true);
    final_update.rows.extend(update(1, false).rows);
    final_update.status = "Ready".into();
    final_update.provider_reset = true;
    final_update.context = Some(Default::default());
    tx.try_send(final_update).unwrap();
    start_receiver(window, rx, cx);
    first_poll(window, cx);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.status, "Synthetic status 0");
            view.stop(cx);
            assert!(view.stopping && !view.busy);
        })
        .unwrap();
    drop(tx);
    settle_turn(window, cx);
    window
        .update(cx, |view, _, cx| {
            let expected: Vec<_> = (0..2).flat_map(|index| update(index, false).rows).collect();
            assert_eq!(view.transcript.read(cx).source_rows(), expected);
            assert_eq!(view.status, "Ready");
            assert!(!view.busy && !view.stopping);
            assert!(view.approvals.is_empty());
            assert_eq!(view.summary.provider_id, None);
            assert_eq!(view.context, Some(Default::default()));
        })
        .unwrap();
}

#[gpui::test]
fn stopped_bot_accepts_a_settled_snapshot_without_its_initial_imported_rows(
    cx: &mut TestAppContext,
) {
    let window = conversation(cx);
    let rows: Vec<_> = [
        ("user", "Previous prompt"),
        ("assistant", "Previous answer"),
        ("user", "First external prompt"),
        ("assistant", "First external answer"),
        ("user", "Second external prompt"),
        ("assistant", "Second external answer"),
        ("user", "Managed prompt"),
        ("assistant", "Settled response"),
    ]
    .into_iter()
    .map(|(role, text)| Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: None,
    })
    .collect();
    let queued = vorkspace_core::follow_up::Message {
        id: "retained-queue".into(),
        text: "Queued after Stop".into(),
        ..Default::default()
    };
    window
        .update(cx, |view, _, cx| {
            view.summary.provider_id = Some("saved-thread".into());
            view.transcript.update(cx, |view, cx| {
                view.bot_mode("cat", 0xe86b5f, "Fixture Bot");
                view.patch(0, rows[..2].to_vec(), cx);
            });
            view.input
                .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
            view.follow_ups.state.messages = vec![queued.clone()];
        })
        .unwrap();

    let (tx, rx) = async_channel::bounded(2);
    start_receiver(window, rx, cx);
    window
        .update(cx, |view, _, cx| {
            view.stop(cx);
            assert!(view.stopping);
            assert_eq!(view.transcript.read(cx).source_rows().len(), 2);
        })
        .unwrap();
    let mut skipped = update(0, false);
    skipped.rows = rows[..7].to_vec();
    skipped.source_len = Some(skipped.rows.len());
    skipped.bot_turn_start = Some(6);
    skipped.provider_id = Some("wrong-skipped-thread".into());
    tx.try_send(skipped).unwrap();
    tx.try_send(vorkspace_runtime::runtime::Update {
        rows: rows.clone(),
        source_len: Some(rows.len()),
        bot_turn_start: Some(6),
        provider_turn_id: Some("managed-turn".into()),
        provider_id: Some("saved-thread".into()),
        status: "Ready".into(),
        finished: true,
        ..Default::default()
    })
    .unwrap();
    drop(tx);
    settle_turn(window, cx);
    window
        .update(cx, |view, _, cx| {
            let transcript = view.transcript.read(cx);
            assert_eq!(transcript.source_rows(), rows);
            assert_eq!(transcript.active_turn(), Some(7));
            assert_eq!(
                transcript.turns[&7].provider_turn_id.as_deref(),
                Some("managed-turn")
            );
            assert!(!transcript.turns.contains_key(&3));
            assert!(!transcript.turns.contains_key(&5));
            assert_eq!(view.summary.provider_id.as_deref(), Some("saved-thread"));
            assert_eq!(view.input.read(cx).content.as_str(), "Unsent draft");
            assert_eq!(view.follow_ups.state.messages, vec![queued]);
            assert!(view.follow_ups.state.paused);
        })
        .unwrap();
}

// Drive the real guarded receiver from the sidebar integration tests.
impl Conversation {
    pub(crate) fn sidebar_test_receiver(
        &mut self,
        cx: &mut Context<Self>,
    ) -> async_channel::Sender<vorkspace_runtime::runtime::Update> {
        self.begin_turn(cx);
        let (tx, rx) = async_channel::bounded(8);
        self.receive_turn_updates(
            rx,
            self.submission_generation,
            self.summary.provider.clone(),
            self.settings.account.id.clone(),
            None,
            cx,
        );
        cx.emit(SessionChanged);
        tx
    }
}

pub(crate) fn sidebar_tool_update(label: Option<&str>) -> vorkspace_runtime::runtime::Update {
    let mut update = update(0, false);
    update.rows.clear();
    update.activity = label.map(str::to_owned);
    update
}

#[gpui::test]
fn recovered_turn_refreshes_the_live_view_and_keeps_the_unsent_composer(cx: &mut TestAppContext) {
    verify_recovered_turn_refresh(cx, false);
}

#[gpui::test]
fn pending_recovered_turn_survives_the_live_history_refresh(cx: &mut TestAppContext) {
    verify_recovered_turn_refresh(cx, true);
}

fn verify_recovered_turn_refresh(cx: &mut TestAppContext, pending: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let mut store = Store::open_vorkspace(&path).unwrap();
    let summary = store.create("codex", "/project").unwrap();
    let id = summary.id;
    let stale = update(0, false).rows;
    let mut saved = stale.clone();
    saved[0].text = "Latest durable history".into();
    store.write_rows(id, 0, &saved).unwrap();
    if pending {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse_recovery BEFORE INSERT ON conversation_turn_recoveries
            BEGIN SELECT RAISE(ABORT,'disk full fixture'); END;",
        )
        .unwrap();
        assert!(
            store
                .save_or_retain_turn_overlap(id, "competing-turn", &stale, None)
                .is_err()
        );
    } else {
        store
            .preserve_turn_overlap(id, "competing-turn", &stale, None)
            .unwrap();
    }
    let losing = stale.clone();
    let store = Arc::new(std::sync::Mutex::new(store));
    let database = store.clone();
    let window = cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, database, cx);
                view.loaded = true;
                view
            })
        })
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            view.transcript
                .update(cx, |view, cx| view.replace_history(stale, cx));
            view.input.update(cx, |input, cx| {
                input.set_text("Unsent draft while streaming".into(), cx)
            });
            view.persist_composer(cx);
        })
        .unwrap();
    let (tx, rx) = async_channel::bounded(1);
    start_receiver(window, rx, cx);
    tx.try_send(vorkspace_runtime::runtime::Update {
        finished: true,
        status: "Recovered a competing version of this turn".into(),
        history_recovery: Some(if pending {
            vorkspace_runtime::runtime::HistoryRecovery::Unsaved
        } else {
            vorkspace_runtime::runtime::HistoryRecovery::Saved
        }),
        ..Default::default()
    })
    .unwrap();
    drop(tx);
    let entity = window.entity(cx).unwrap();
    let executor = cx.foreground_executor().clone();
    executor.block_test(cx.condition(&entity, |view, cx| {
        !view.busy
            && !view.loading
            && view.transcript.read(cx).source_rows()[0].text == "Latest durable history"
    }));
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.transcript.read(cx).source_rows(), saved);
            assert_eq!(
                view.input.read(cx).content.as_str(),
                "Unsent draft while streaming"
            );
            assert!(view.recovered_turn_banner(cx).is_some());
            let store = locked(&store).unwrap();
            assert_eq!(store.recovered_turns(id).unwrap()[0].pending, pending);
            assert_eq!(
                store.read_recovered_turn(id, "competing-turn").unwrap(),
                Some(losing)
            );
        })
        .unwrap();
}
