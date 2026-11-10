//! Keep a new durable session private until its creator installs its view.
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone, Default)]
pub(crate) struct Registry(Arc<Mutex<HashSet<i64>>>);

impl Registry {
    pub(crate) fn begin(&self, pending: &Arc<AtomicUsize>) -> Install {
        pending.fetch_add(1, Ordering::Relaxed);
        Install {
            registry: self.clone(),
            pending: pending.clone(),
            id: None,
        }
    }

    pub(crate) fn contains(&self, id: i64) -> bool {
        self.0.lock().unwrap().contains(&id)
    }

    pub(crate) fn visible_sessions(
        &self,
        db: &vorkspace_core::store::Store,
    ) -> Result<Vec<vorkspace_core::store::Session>, String> {
        let mut sessions = db.sessions()?;
        let roster = vorkspace_core::bot::Roster::load(db)?;
        let internal: HashSet<_> = roster
            .hidden_sessions
            .into_iter()
            .chain(roster.bots.iter().flat_map(|bot| {
                bot.session_id
                    .into_iter()
                    .chain(bot.habit_sessions.iter().copied())
            }))
            .collect();
        let pending = self.0.lock().unwrap();
        sessions
            .retain(|session| !pending.contains(&session.id) && !internal.contains(&session.id));
        Ok(sessions)
    }
}

pub(crate) struct Install {
    registry: Registry,
    pending: Arc<AtomicUsize>,
    id: Option<i64>,
}

impl Install {
    /// Register under the same database guard that created the row.
    pub(crate) fn track(&mut self, id: i64) {
        assert!(self.id.is_none());
        self.registry.0.lock().unwrap().insert(id);
        self.id = Some(id);
    }

    /// Admission and publication are synchronous from this point in the UI.
    /// Keep the preparation count until this ticket leaves the callback.
    pub(crate) fn expose(&mut self) {
        if let Some(id) = self.id.take() {
            self.registry.0.lock().unwrap().remove(&id);
        }
    }
}

impl Drop for Install {
    fn drop(&mut self) {
        self.expose();
        self.pending.fetch_sub(1, Ordering::Relaxed);
    }
}
