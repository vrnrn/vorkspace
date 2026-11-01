//! Application-owned card saves outlive views. Admission never takes the Store
//! lock. One background consumer serializes CAS writes and coalesces each card.
use super::{Database, locked};
use gpui::{App, Global};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex, PoisonError},
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
                        let mut queue = work.queue.lock().unwrap_or_else(PoisonError::into_inner);
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
                    work.queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .finish(job, result);
                    #[cfg(any(test, feature = "test-support"))]
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
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
        self.shared.wake.notify_one();
        self.finished.clone()
    }
    #[cfg(any(test, feature = "test-support"))]
    fn shutdown(&self) {
        let _ = self.close().recv_blocking();
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
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
        self.thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
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

pub struct Ticket {
    pub version: u64,
    key: Key,
    shared: Arc<Shared>,
    receiver: async_channel::Receiver<Reply>,
}
impl Ticket {
    pub async fn recv(&self) -> Result<Reply, async_channel::RecvError> {
        self.receiver.recv().await
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.receiver.close();
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if queue
            .entries
            .get(&self.key)
            .is_some_and(|e| e.version == self.version && e.settled)
        {
            queue.entries.remove(&self.key);
        }
    }
}

pub fn admit(
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

#[cfg(any(test, feature = "test-support"))]
pub fn wait_for_saves_for_test(cx: &App) {
    if let Some(owner) = cx.try_global::<Owner>() {
        let shared = &owner.worker.shared;
        let mut queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
        while !queue.order.is_empty() || queue.entries.values().any(|entry| !entry.settled) {
            queue = shared.wake.wait(queue).unwrap();
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn finish_for_test(cx: &App) {
    if let Some(owner) = cx.try_global::<Owner>() {
        owner.worker.shutdown();
    }
}

#[cfg(test)]
mod tests;
