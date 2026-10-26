//! Application-owned card saves outlive views. Admission never takes the Store
//! lock. One background consumer serializes CAS writes and coalesces each card.
use super::{Database, locked};
use gpui::{App, Global};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex},
};
use vorkspace_core::transcript::Row;

type Reply = Result<Option<Row>, String>;
type Key = (usize, i64, String);
struct Entry {
    store: Database,
    origin: Value,
    expected: Value,
    desired: Value,
    version: u64,
    settled: bool,
    reply: async_channel::Sender<Reply>,
}
#[derive(Default)]
struct Queue {
    closed: bool,
    version: u64,
    order: VecDeque<Key>,
    entries: HashMap<Key, Entry>,
}
struct Job {
    key: Key,
    store: Database,
    expected: Value,
    desired: Value,
    version: u64,
}
impl Job {
    fn save(&self) -> Reply {
        // An unchanged admission is free, including no database mutex wait.
        if self.expected == self.desired {
            return Ok(None);
        }
        locked(&self.store)?.refresh_bot_card(self.key.1, &self.expected, &self.desired)
    }
}
impl Queue {
    fn admit(
        &mut self,
        store: Database,
        session: i64,
        expected: Value,
        desired: Value,
    ) -> Result<Option<(u64, async_channel::Receiver<Reply>)>, String> {
        if self.closed {
            return Err("Card persistence is shutting down".into());
        }
        let id = expected["id"]
            .as_str()
            .ok_or("Card is missing its identity")?
            .to_owned();
        let key = (Arc::as_ptr(&store) as usize, session, id);
        if expected == desired && !self.entries.contains_key(&key) {
            return Ok(None);
        }
        if let Some(entry) = self.entries.get(&key) {
            if entry.origin != expected && entry.expected != expected {
                return Err("Card changed while a refresh was pending".into());
            }
            if entry.desired == desired {
                return Ok(None);
            }
        }
        self.version += 1;
        let version = self.version;
        let (reply, receiver) = async_channel::bounded(1);
        if let Some(entry) = self.entries.get_mut(&key) {
            // Dropping the previous reply suppresses superseded publication.
            if entry.settled {
                entry.settled = false;
                self.order.push_back(key.clone());
            }
            entry.desired = desired;
            entry.version = version;
            entry.reply = reply;
        } else {
            self.order.push_back(key.clone());
            self.entries.insert(
                key,
                Entry {
                    store,
                    origin: expected.clone(),
                    expected,
                    desired,
                    version,
                    settled: false,
                    reply,
                },
            );
        }
        Ok(Some((version, receiver)))
    }
    fn next(&mut self) -> Option<Job> {
        let key = self.order.pop_front()?;
        let entry = self.entries.get(&key)?;
        Some(Job {
            key,
            store: entry.store.clone(),
            expected: entry.expected.clone(),
            desired: entry.desired.clone(),
            version: entry.version,
        })
    }
    fn finish(&mut self, job: Job, result: Reply) {
        let entry = self.entries.get_mut(&job.key).unwrap();
        let success =
            matches!(result, Ok(Some(_))) || (job.expected == job.desired && result.is_ok());
        if entry.version != job.version && success {
            // Only our successful CAS may advance the base of a superseding
            // update. External changes and failed writes never get rebased.
            entry.expected = job.desired;
            self.order.push_back(job.key);
        } else if success && !entry.reply.is_closed() {
            entry.expected = job.desired;
            entry.settled = true;
            let _ = entry.reply.try_send(result);
            // Retain the successful base until the UI observer has handled its
            // receipt. Admission in the commit -> publication gap must rebase
            // through our own write, never race it with the old visible value.
        } else {
            let entry = self.entries.remove(&job.key).unwrap();
            let _ = entry.reply.try_send(result);
        }
    }
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}
struct Worker {
    shared: Arc<Shared>,
    finished: async_channel::Receiver<()>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl Worker {
    fn start() -> Result<Arc<Self>, String> {
        let shared = Arc::new(Shared::default());
        let work = shared.clone();
        let (done, finished) = async_channel::bounded(1);
        let thread = std::thread::Builder::new()
            .name("bot-card-save".into())
            .spawn(move || {
                loop {
                    let job = {
                        let mut queue = work.queue.lock().unwrap();
                        while queue.order.is_empty() && !queue.closed {
                            queue = work.wake.wait(queue).unwrap();
                        }
                        match queue.next() {
                            Some(job) => job,
                            None if queue.closed => break,
                            None => continue,
                        }
                    };
                    let result = job.save();
                    work.queue.lock().unwrap().finish(job, result);
                    #[cfg(test)]
                    work.wake.notify_all();
                }
                let _ = done.try_send(());
            })
            .map_err(|_| "Card persistence worker unavailable".to_owned())?;
        Ok(Arc::new(Self {
            shared,
            finished,
            thread: Mutex::new(Some(thread)),
        }))
    }
    fn close(&self) -> async_channel::Receiver<()> {
        self.shared.queue.lock().unwrap().closed = true;
        self.shared.wake.notify_one();
        self.finished.clone()
    }
    #[cfg(test)]
    fn shutdown(&self) {
        let _ = self.close().recv_blocking();
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.close();
        // Dropping a JoinHandle detaches; the consumer owns its queue and can
        // finish independently. Never wait for storage on the foreground,
        // including owner destruction after GPUI's quit deadline expires.
        self.thread.lock().unwrap().take();
    }
}
struct Owner {
    worker: Arc<Worker>,
    _quit: gpui::Subscription,
}
impl Global for Owner {}

fn install(cx: &mut App) -> Result<(), String> {
    if cx.try_global::<Owner>().is_some() {
        return Ok(());
    }
    let worker = Worker::start()?;
    let quitting = Arc::downgrade(&worker);
    let quit = cx.on_app_quit(move |_| {
        let finished = quitting.upgrade().map(|worker| worker.close());
        // Drain within GPUI's existing asynchronous quit budget. A cosmetic
        // refresh that cannot commit before exit remains unpublished; its
        // previous durable value survives and can be refreshed next launch.
        async move {
            if let Some(finished) = finished {
                let _ = finished.recv().await;
            }
        }
    });
    cx.set_global(Owner {
        worker,
        _quit: quit,
    });
    Ok(())
}

pub(crate) struct Ticket {
    pub(crate) version: u64,
    key: Key,
    shared: Arc<Shared>,
    receiver: async_channel::Receiver<Reply>,
}
impl Ticket {
    pub(crate) async fn recv(&self) -> Result<Reply, async_channel::RecvError> {
        self.receiver.recv().await
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.receiver.close();
        let mut queue = self.shared.queue.lock().unwrap();
        if queue
            .entries
            .get(&self.key)
            .is_some_and(|e| e.version == self.version && e.settled)
        {
            queue.entries.remove(&self.key);
        }
    }
}

pub(crate) fn admit(
    cx: &mut App,
    store: Database,
    session: i64,
    expected: Value,
    desired: Value,
) -> Result<Option<Ticket>, String> {
    install(cx)?;
    let owner = cx.global::<Owner>();
    let key = (
        Arc::as_ptr(&store) as usize,
        session,
        expected["id"]
            .as_str()
            .ok_or("Card is missing its identity")?
            .to_owned(),
    );
    let admitted = owner
        .worker
        .shared
        .queue
        .lock()
        .unwrap()
        .admit(store, session, expected, desired)?;
    if admitted.is_some() {
        owner.worker.shared.wake.notify_one();
    }
    Ok(admitted.map(|(version, receiver)| Ticket {
        version,
        key,
        shared: owner.worker.shared.clone(),
        receiver,
    }))
}

#[cfg(test)]
pub(crate) fn wait_for_saves_for_test(cx: &App) {
    if let Some(owner) = cx.try_global::<Owner>() {
        let shared = &owner.worker.shared;
        let mut queue = shared.queue.lock().unwrap();
        while !queue.order.is_empty() || queue.entries.values().any(|entry| !entry.settled) {
            queue = shared.wake.wait(queue).unwrap();
        }
    }
}

#[cfg(test)]
pub(crate) fn finish_for_test(cx: &App) {
    if let Some(owner) = cx.try_global::<Owner>() {
        owner.worker.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vorkspace_core::{store::Store, transcript::Transcript};

    fn fixture() -> (Database, i64, Value) {
        let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let id = store.create("codex", "/synthetic-card-project").unwrap().id;
        let value =
            json!({"id":"card","type":"pr","repo":"synthetic/repo","number":1,"title":"Old"});
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
}
