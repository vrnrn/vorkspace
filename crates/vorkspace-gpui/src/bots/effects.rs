//! Revocation rejects admitted effects which have not started. Retirement
//! waits for executing effects on the background executor before acknowledging
//! the change; control actions never wait on file locks on the UI thread.
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use vorkspace_core::bot::Roster;

#[derive(Clone)]
pub(super) struct Permit(Arc<(AtomicBool, Mutex<()>)>);

impl Permit {
    fn new() -> Self {
        Self(Arc::new((AtomicBool::new(true), Mutex::new(()))))
    }
    fn revoke(&self) {
        self.0.0.store(false, Ordering::Release);
    }
    pub(super) fn wait(&self) -> Result<(), String> {
        let guard = self
            .0
            .1
            .lock()
            .map_err(|_| "Minion effect fence is unavailable")?;
        drop(guard);
        Ok(())
    }
    pub(super) fn run<T>(&self, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let active = self
            .0
            .1
            .lock()
            .map_err(|_| "Minion authority is unavailable")?;
        if !self.0.0.load(Ordering::Acquire) {
            return Err("Minion authority changed before the action executed".into());
        }
        let result = work();
        drop(active);
        result
    }
}

#[derive(Default)]
pub(super) struct MinionEffects {
    active: HashMap<String, (Vec<String>, Permit)>,
    previous: HashMap<String, Vec<Permit>>,
}

impl MinionEffects {
    pub(super) fn reconcile(&mut self, roster: &Roster, enabled: bool) {
        self.previous.retain(|_, permits| {
            permits.retain(|p| Arc::strong_count(&p.0) > 1);
            !permits.is_empty()
        });
        self.active.retain(|id, (projects, permit)| {
            let keep = enabled
                && roster.is_active(id)
                && roster
                    .storage_owner(id)
                    .is_some_and(|p| p.projects == *projects);
            if !keep {
                permit.revoke();
                self.previous
                    .entry(id.clone())
                    .or_default()
                    .push(permit.clone());
            }
            keep
        });
        if enabled {
            for member in roster.bots.iter().filter(|b| b.is_minion()) {
                if roster.is_active(&member.id)
                    && let Some(owner) = roster.storage_owner(&member.id)
                {
                    self.active
                        .entry(member.id.clone())
                        .or_insert_with(|| (owner.projects.clone(), Permit::new()));
                }
            }
        }
    }
    pub(super) fn permit(&self, actor: &str) -> Result<Permit, String> {
        self.active
            .get(actor)
            .map(|(_, permit)| permit.clone())
            .ok_or_else(|| "Minion no longer owns an active app capability".into())
    }
    pub(super) fn fences(&self, actor: &str) -> Vec<Permit> {
        self.previous
            .get(actor)
            .into_iter()
            .flatten()
            .cloned()
            .chain(self.active.get(actor).map(|(_, p)| p.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vorkspace_core::bot::Bot;

    #[test]
    fn retirement_revokes_immediately_and_fences_an_executing_effect() {
        let permit = Permit::new();
        let (entered, started) = std::sync::mpsc::channel();
        let (release, proceed) = std::sync::mpsc::channel();
        let changed = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let work = permit.clone();
            let changed = &changed;
            scope.spawn(move || {
                work.run(|| {
                    entered.send(()).unwrap();
                    proceed.recv().unwrap();
                    changed.store(true, Ordering::Release);
                    Ok(())
                })
                .unwrap()
            });
            started.recv().unwrap();
            // This must return while the effect owns its lock.
            permit.revoke();
            release.send(()).unwrap();
            permit.wait().unwrap();
            assert!(changed.load(Ordering::Acquire));
            assert!(permit.run(|| Ok(())).is_err());
        });
    }

    #[test]
    fn queued_effects_are_revoked_on_retirement_scope_changes_and_disable() {
        let mut roster = Roster::default();
        roster.bots.push(Bot {
            id: "parent".into(),
            ..Default::default()
        });
        let id = roster
            .hire_minion("parent", "Scout", "Review", vec![], None)
            .unwrap();
        let mut effects = MinionEffects::default();
        effects.reconcile(&roster, true);
        let old = effects.permit(&id).unwrap();
        effects.reconcile(&roster, true);
        assert_eq!(old.run(|| Ok(7)).unwrap(), 7);
        roster.get_mut("parent").unwrap().projects = vec!["/new-scope".into()];
        effects.reconcile(&roster, true);
        let mut executed = false;
        assert!(
            old.run(|| {
                executed = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!executed);
        let next = effects.permit(&id).unwrap();
        assert!(next.run(|| Ok(())).is_ok());
        effects.reconcile(&roster, false);
        assert!(next.run(|| Ok(())).is_err());
        effects.reconcile(&roster, true);
        let retired = effects.permit(&id).unwrap();
        roster.retire_minion("parent", &id).unwrap();
        effects.reconcile(&roster, true);
        assert!(retired.run(|| Ok(())).is_err());
        assert!(effects.permit(&id).is_err());
    }
}
