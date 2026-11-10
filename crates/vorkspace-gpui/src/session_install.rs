//! Keep a new durable session private until its creator installs its view.
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone, Default)]
pub(crate) struct Registry(Arc<Mutex<HashMap<i64, Arc<()>>>>);

impl Registry {
    pub(crate) fn begin(&self, pending: &Arc<AtomicUsize>) -> Install {
        pending.fetch_add(1, Ordering::Relaxed);
        Install {
            registry: self.clone(),
            pending: pending.clone(),
            id: None,
            ticket: Arc::new(()),
        }
    }

    pub(crate) fn permits(&self, id: i64, install: Option<&Install>) -> bool {
        let pending = self.0.lock().unwrap();
        if let Some(install) = install.filter(|install| install.id.is_some()) {
            return pending
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(&install.ticket, current));
        }
        !pending.contains_key(&id)
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
        sessions.retain(|session| {
            !pending.contains_key(&session.id) && !internal.contains(&session.id)
        });
        Ok(sessions)
    }
}

pub(crate) struct Install {
    registry: Registry,
    pending: Arc<AtomicUsize>,
    id: Option<i64>,
    ticket: Arc<()>,
}

impl Install {
    /// Register under the same database guard that created the row.
    pub(crate) fn track(&mut self, id: i64) {
        assert!(self.id.is_none());
        // Legacy SQLite stores can allocate an ID again after a deletion.
        self.registry
            .0
            .lock()
            .unwrap()
            .insert(id, self.ticket.clone());
        self.id = Some(id);
    }

    /// Admission and publication are synchronous from this point in the UI.
    /// Keep the preparation count until this ticket leaves the callback.
    pub(crate) fn expose(&mut self) {
        if let Some(id) = self.id.take() {
            let mut pending = self.registry.0.lock().unwrap();
            if pending
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &self.ticket))
            {
                pending.remove(&id);
            }
        }
    }

    /// Delete and release while still holding the store guard. Another
    /// allocation must never acquire the old reservation or be rolled back by it.
    pub(crate) fn discard(
        &mut self,
        db: &mut vorkspace_core::store::Store,
        id: i64,
    ) -> Result<(), String> {
        let owns = self.id == Some(id)
            && self
                .registry
                .0
                .lock()
                .unwrap()
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &self.ticket));
        if !owns {
            return Err(
                "The prepared conversation was replaced; its current contents were preserved"
                    .into(),
            );
        }
        db.delete(id)?;
        self.expose();
        Ok(())
    }
}

impl Drop for Install {
    fn drop(&mut self) {
        self.expose();
        self.pending.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use vorkspace_core::store::Store;

    #[test]
    fn a_reused_sqlite_row_cannot_be_exposed_or_deleted_by_its_previous_ticket() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let registry = Registry::default();
        let pending = Arc::new(AtomicUsize::new(0));
        let mut old = registry.begin(&pending);
        let id = store.create("codex", "/project").unwrap().id;
        old.track(id);
        store.delete(id).unwrap();
        let mut new = registry.begin(&pending);
        let replacement = store.create("codex", "/project").unwrap().id;
        assert_eq!(id, replacement);
        new.track(replacement);
        assert!(!registry.permits(id, Some(&old)));
        assert!(old.discard(&mut store, id).is_err());
        drop(old);
        assert!(!registry.permits(id, None));
        assert!(registry.permits(id, Some(&new)));
        assert!(store.bot_session(id).unwrap().is_some());
        new.discard(&mut store, id).unwrap();
        assert!(registry.permits(id, None));
        assert!(store.bot_session(id).unwrap().is_none());
    }
}
