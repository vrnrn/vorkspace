use super::*;
use serde_json::json;
use vorkspace_core::{store::Store, transcript::Transcript};

fn fixture() -> (Database, i64, Value) {
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let id = store.create("codex", "/synthetic-card-project").unwrap().id;
    let value = json!({"id":"card","type":"pr","repo":"synthetic/repo","number":1,"title":"Old"});
    let row = Row {
        role: "bot-card".into(),
        text: value.to_string(),
        first: true,
        sent_at: Some(123),
    };
    store.write_rows(id, 0, &[row]).unwrap();
    (Arc::new(Mutex::new(store)), id, value)
}
fn title(base: &Value, title: &str) -> Value {
    let mut next = base.clone();
    next["title"] = json!(title);
    next
}
fn saved(store: &Database, id: i64) -> Value {
    serde_json::from_str(&locked(store).unwrap().load(id).unwrap().rows[0].text).unwrap()
}
fn complete(queue: &mut Queue) {
    let job = queue.next().unwrap();
    let result = job.save();
    queue.finish(job, result);
}

#[test]
fn delayed_write_coalesces_and_rebases_only_its_own_successful_commit() {
    let (store, id, base) = fixture();
    let mut queue = Queue::default();
    let (_, old_reply) = queue
        .admit(store.clone(), id, base.clone(), title(&base, "One"))
        .unwrap()
        .unwrap();
    let first = queue.next().unwrap();
    // Hold persistence at a deterministic boundary; the UI still admits
    // newer values without trying to acquire the held database mutex.
    let guard = locked(&store).unwrap();
    let (started, ready) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        started.send(()).unwrap();
        let result = first.save();
        (first, result)
    });
    ready.recv().unwrap();
    let (_, middle) = queue
        .admit(store.clone(), id, base.clone(), title(&base, "Two"))
        .unwrap()
        .unwrap();
    let (_, latest) = queue
        .admit(store.clone(), id, base.clone(), title(&base, "Three"))
        .unwrap()
        .unwrap();
    assert!(old_reply.is_closed());
    assert!(middle.is_closed());
    assert!(latest.try_recv().is_err());
    drop(guard);
    let (job, result) = thread.join().unwrap();
    queue.finish(job, result);
    assert!(
        latest.try_recv().is_err(),
        "A superseded completion cannot publish"
    );
    complete(&mut queue);
    let row = latest.try_recv().unwrap().unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&row.text).unwrap()["title"],
        "Three"
    );
    assert_eq!(saved(&store, id)["title"], "Three");
    assert!(queue.order.is_empty());
}

#[test]
fn commit_to_publication_gap_and_reversion_keep_the_latest_value() {
    let (store, id, base) = fixture();
    let mut queue = Queue::default();
    let (_, first) = queue
        .admit(store.clone(), id, base.clone(), title(&base, "One"))
        .unwrap()
        .unwrap();
    complete(&mut queue);
    assert!(first.try_recv().unwrap().unwrap().is_some());
    // UI still displays the old base; its newer refresh reverts to that
    // value before the first receipt's publication callback has run.
    let (_, reverted) = queue
        .admit(store.clone(), id, base.clone(), base.clone())
        .unwrap()
        .unwrap();
    complete(&mut queue);
    assert!(reverted.try_recv().unwrap().unwrap().is_some());
    assert_eq!(saved(&store, id), base);
}

#[test]
fn stale_external_card_and_store_failures_release_admission_without_rebasing() {
    let (store, id, base) = fixture();
    let mut queue = Queue::default();
    let (_, old) = queue
        .admit(store.clone(), id, base.clone(), title(&base, "One"))
        .unwrap()
        .unwrap();
    let job = queue.next().unwrap();
    let (_, newest) = queue
        .admit(store.clone(), id, base.clone(), title(&base, "Two"))
        .unwrap()
        .unwrap();
    let external = title(&base, "External");
    locked(&store)
        .unwrap()
        .refresh_bot_card(id, &base, &external)
        .unwrap()
        .unwrap();
    let result = job.save();
    queue.finish(job, result);
    assert!(old.is_closed());
    assert!(newest.try_recv().unwrap().unwrap().is_none());
    assert_eq!(saved(&store, id), external);
    assert!(queue.entries.is_empty());
    let (_, failed) = queue
        .admit(store.clone(), id, external.clone(), title(&base, "Retry"))
        .unwrap()
        .unwrap();
    let job = queue.next().unwrap();
    queue.finish(job, Err("synthetic failure".into()));
    assert!(failed.try_recv().unwrap().is_err());
    assert!(queue.entries.is_empty());
    let (_, retried) = queue
        .admit(store.clone(), id, external.clone(), title(&base, "Retry"))
        .unwrap()
        .unwrap();
    complete(&mut queue);
    assert!(retried.try_recv().unwrap().unwrap().is_some());
}

#[test]
fn poison_is_an_error_and_unchanged_updates_do_not_even_wait_for_the_store() {
    let (store, id, base) = fixture();
    let mut queue = Queue::default();
    let guard = locked(&store).unwrap();
    assert!(
        queue
            .admit(store.clone(), id, base.clone(), base.clone())
            .unwrap()
            .is_none()
    );
    assert!(queue.entries.is_empty());
    drop(guard);
    let poison = store.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = poison.lock().unwrap();
            panic!("synthetic poison")
        })
        .join()
        .is_err()
    );
    let (_, reply) = queue
        .admit(store, id, base.clone(), title(&base, "New"))
        .unwrap()
        .unwrap();
    complete(&mut queue);
    assert!(reply.try_recv().unwrap().is_err());
    assert!(queue.entries.is_empty());
}

#[test]
fn application_worker_drains_after_observer_drop_and_rejects_shutdown_admission() {
    let (store, id, base) = fixture();
    let worker = Worker::start().unwrap();
    let guard = locked(&store).unwrap();
    let (_, observer) = worker
        .shared
        .queue
        .lock()
        .unwrap()
        .admit(store.clone(), id, base.clone(), title(&base, "After drop"))
        .unwrap()
        .unwrap();
    worker.shared.wake.notify_one();
    drop(observer); // Navigation/entity lifetime cannot cancel durable work.
    worker.shared.queue.lock().unwrap().closed = true;
    assert!(
        worker
            .shared
            .queue
            .lock()
            .unwrap()
            .admit(store.clone(), id, base.clone(), base)
            .is_err()
    );
    drop(guard);
    worker.shutdown(); // Joins the exact owned thread, including a pending save.
    assert_eq!(saved(&store, id)["title"], "After drop");
    assert!(worker.shared.queue.lock().unwrap().entries.is_empty());
    assert!(worker.thread.lock().unwrap().is_none());
}

#[test]
fn closing_and_dropping_the_owner_never_wait_for_a_held_store() {
    let (store, id, base) = fixture();
    let worker = Worker::start().unwrap();
    let guard = locked(&store).unwrap();
    let (_, observer) = worker
        .shared
        .queue
        .lock()
        .unwrap()
        .admit(store.clone(), id, base.clone(), title(&base, "After close"))
        .unwrap()
        .unwrap();
    worker.shared.wake.notify_one();
    let finished = worker.close();
    assert!(finished.try_recv().is_err());
    drop(worker);
    // Both operations returned while this thread still owns the database
    // lock. The detached consumer retains and drains the admitted save.
    drop(guard);
    finished.recv_blocking().unwrap();
    assert!(observer.recv_blocking().unwrap().unwrap().is_some());
    assert_eq!(saved(&store, id)["title"], "After close");
}

#[test]
fn ticket_drop_releases_both_inflight_and_settled_observation_state() {
    for finish_first in [false, true] {
        let (store, id, base) = fixture();
        let shared = Arc::new(Shared::default());
        let (version, receiver) = shared
            .queue
            .lock()
            .unwrap()
            .admit(store, id, base.clone(), title(&base, "New"))
            .unwrap()
            .unwrap();
        let job = shared.queue.lock().unwrap().next().unwrap();
        let ticket = Ticket {
            version,
            key: job.key.clone(),
            shared: shared.clone(),
            receiver,
        };
        if finish_first {
            let result = job.save();
            shared.queue.lock().unwrap().finish(job, result);
            drop(ticket);
        } else {
            drop(ticket);
            let result = job.save();
            shared.queue.lock().unwrap().finish(job, result);
        }
        assert!(shared.queue.lock().unwrap().entries.is_empty());
    }
}

#[test]
fn append_and_queue_delivery_are_not_admitted_through_the_card_worker() {
    let (store, id, base) = fixture();
    let mut queue = Queue::default();
    let (_, reply) = queue
        .admit(store.clone(), id, base.clone(), title(&base, "New"))
        .unwrap()
        .unwrap();
    let job = queue.next().unwrap();
    let mut history: Transcript = locked(&store).unwrap().load(id).unwrap();
    let from = history.begin("user");
    history.append("Synthetic queued prompt");
    locked(&store)
        .unwrap()
        .write_rows_with_message(id, from, &history.rows[from..], Some("queued-receipt"))
        .unwrap();
    let result = job.save();
    queue.finish(job, result);
    assert!(reply.try_recv().unwrap().unwrap().is_some());
    let store = locked(&store).unwrap();
    assert_eq!(store.load(id).unwrap().rows[1..], history.rows[1..]);
    assert_eq!(
        store
            .setting(&format!("bot-delivered:{id}:queued-receipt"))
            .unwrap(),
        Some("1".into())
    );
}
