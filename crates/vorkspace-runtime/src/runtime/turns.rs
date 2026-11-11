//! Runtime admission and exact Stop capabilities outlive optional view projection.
use super::{Database, TurnInput, TurnOutcome, TurnTransition, Update, manual_compaction};
use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
};
use vorkspace_core::{
    agent::{Control, Settings},
    process::TurnToken,
    remote::Connections,
    store::Session,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionKey {
    database: usize,
    session: i64,
}

impl SessionKey {
    pub fn new(database: &Database, session: i64) -> Self {
        Self {
            database: Arc::as_ptr(database) as usize,
            session,
        }
    }
}

type ActiveTurns = HashMap<SessionKey, Arc<AtomicBool>>;

/// Share one registry at the application composition boundary.
#[derive(Clone, Default)]
pub struct Runs(Arc<Mutex<ActiveTurns>>);

impl Runs {
    fn active(&self) -> MutexGuard<'_, ActiveTurns> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn available(&self, database: &Database, session: i64) -> bool {
        !self
            .active()
            .contains_key(&SessionKey::new(database, session))
    }

    pub fn owns(&self, database: &Database, session: i64, token: &TurnToken) -> bool {
        self.active()
            .get(&SessionKey::new(database, session))
            .is_some_and(|cancel| token.matches(cancel))
    }

    pub fn admit(&self, database: &Database, session: i64) -> Option<Admission> {
        let key = SessionKey::new(database, session);
        let mut active = self.active();
        let Entry::Vacant(slot) = active.entry(key) else {
            return None;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        slot.insert(cancel.clone());
        Some(Admission {
            owner: self.clone(),
            key,
            database: database.clone(),
            cancel,
        })
    }

    pub fn stop(&self, database: &Database, session: i64, token: &TurnToken) -> bool {
        let active = self.active();
        let Some(cancel) = active
            .get(&SessionKey::new(database, session))
            .filter(|cancel| token.matches(cancel))
        else {
            return false;
        };
        cancel.store(true, Ordering::Relaxed);
        true
    }
}

/// Move this guard to the worker before spawning its operation. Dropping a
/// subscription never releases execution admission. The worker's last save,
/// return or unwind releases it through this guard.
pub struct Admission {
    owner: Runs,
    key: SessionKey,
    database: Database,
    cancel: Arc<AtomicBool>,
}

impl Admission {
    pub fn key(&self) -> SessionKey {
        self.key
    }

    pub fn token(&self) -> TurnToken {
        (&self.cancel).into()
    }

    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub fn run(
        self,
        input: TurnInput,
        remote: Option<Arc<Connections>>,
        controls: Receiver<Control>,
        updates: async_channel::Sender<Update>,
    ) {
        if input.summary.id != self.key.session {
            let _ = updates.send_blocking(Update {
                transition: TurnTransition::settled(TurnOutcome::Failed),
                status: "The turn admission belongs to another conversation.".into(),
                ..Default::default()
            });
            return;
        }
        if let Some(client) = remote {
            super::remote::run_with_client(
                client,
                input,
                controls,
                self.database.clone(),
                self.cancel.clone(),
                updates,
            );
        } else {
            super::run_turn(
                input,
                controls,
                self.database.clone(),
                self.cancel.clone(),
                updates,
            );
        }
    }

    pub fn compact(
        self,
        summary: Session,
        settings: Settings,
        started: impl FnOnce(manual_compaction::Started),
    ) -> manual_compaction::Finished {
        if summary.id != self.key.session {
            return manual_compaction::Finished {
                result: Err("The turn admission belongs to another conversation.".into()),
                row: None,
                context: None,
                commands: None,
            };
        }
        manual_compaction::run(
            summary,
            settings,
            self.database.clone(),
            self.cancel.clone(),
            started,
        )
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        let mut active = self.owner.active();
        if active
            .get(&self.key)
            .is_some_and(|cancel| Arc::ptr_eq(cancel, &self.cancel))
        {
            active.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> (Database, Session) {
        let store = vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/fixture").unwrap();
        (Arc::new(Mutex::new(store)), session)
    }

    #[test]
    fn admission_is_database_scoped_and_unwind_releases_only_its_run() {
        let (first, session) = database();
        let (second, _) = database();
        let runs = Runs::default();
        let admission = runs.admit(&first, session.id).unwrap();
        assert!(runs.admit(&first, session.id).is_none());
        assert!(runs.available(&second, session.id));
        let prior = admission.token();
        let other = runs.admit(&second, session.id).unwrap();
        assert!(!runs.stop(&second, session.id, &prior));
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _admission = admission;
            panic!("controlled worker unwind");
        }));
        assert!(panic.is_err());
        assert!(runs.available(&first, session.id));
        assert!(!runs.available(&second, session.id));
        let successor = runs.admit(&first, session.id).unwrap();
        assert!(!runs.stop(&first, session.id, &prior));
        assert!(!successor.cancel_handle().load(Ordering::Relaxed));
        drop(other);
        drop(successor);
        assert!(runs.available(&first, session.id));
        assert!(runs.available(&second, session.id));
    }

    #[test]
    fn worker_owns_admission_and_stop_through_its_last_durable_effect() {
        let (database, session) = database();
        let runs = Runs::default();
        let admission = runs.admit(&database, session.id).unwrap();
        let token = admission.token();
        let (ready, waiting) = std::sync::mpsc::channel();
        let (release, finish) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            ready.send(()).unwrap();
            finish.recv().unwrap();
            assert!(admission.cancel.load(Ordering::Relaxed));
            super::super::locked(&admission.database)
                .unwrap()
                .save_setting("last-effect", "saved")
                .unwrap();
            drop(admission);
        });
        waiting.recv().unwrap();
        assert!(runs.stop(&database, session.id, &token));
        assert!(!runs.available(&database, session.id));
        assert_eq!(
            super::super::locked(&database)
                .unwrap()
                .setting("last-effect")
                .unwrap(),
            None
        );
        release.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(
            super::super::locked(&database)
                .unwrap()
                .setting("last-effect")
                .unwrap()
                .as_deref(),
            Some("saved")
        );
        assert!(runs.available(&database, session.id));
        let successor = runs.admit(&database, session.id).unwrap();
        assert!(!runs.stop(&database, session.id, &token));
        assert!(runs.owns(&database, session.id, &successor.token()));
    }

    #[test]
    fn an_admission_cannot_dispatch_or_compact_another_conversation() {
        let (database, source) = database();
        let target = super::super::locked(&database)
            .unwrap()
            .create("codex", "/other")
            .unwrap();
        let runs = Runs::default();
        let admission = runs.admit(&database, source.id).unwrap();
        let (_, controls) = std::sync::mpsc::channel();
        let (updates, result) = async_channel::bounded(1);
        admission.run(
            TurnInput {
                summary: target.clone(),
                prompt: "Never dispatched".into(),
                settings: Settings::default(),
                composer_origin: None,
            },
            None,
            controls,
            updates,
        );
        let update = result.recv_blocking().unwrap();
        assert_eq!(update.settlement().unwrap().outcome, TurnOutcome::Failed);
        assert!(update.status.contains("admission belongs"));
        assert!(runs.available(&database, source.id));
        let admission = runs.admit(&database, source.id).unwrap();
        let compact = admission.compact(target.clone(), Settings::default(), |_| {
            panic!("must not start")
        });
        assert!(compact.result.unwrap_err().contains("admission belongs"));
        assert!(runs.available(&database, source.id));
        assert!(
            super::super::locked(&database)
                .unwrap()
                .load(target.id)
                .unwrap()
                .rows
                .is_empty()
        );
    }
}
