//! Application-owned admission, worker lifetime and delivery to the live view.
use super::{Conversation, turn_projection::TurnProjection};
use gpui::{App, Entity, Global, WeakEntity};
use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
};
use vorkspace_core::{agent::Control, process::TurnToken, remote::Connections};
use vorkspace_runtime::runtime::{self, Database, Update};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SessionKey {
    store: usize,
    session: i64,
}

impl SessionKey {
    fn new(store: &Database, session: i64) -> Self {
        Self {
            store: Arc::as_ptr(store) as usize,
            session,
        }
    }
}

struct Active {
    token: TurnToken,
    view: WeakEntity<Conversation>,
}

#[derive(Default)]
struct Runs(HashMap<SessionKey, Active>);
impl Global for Runs {}

pub(super) const ALREADY_ACTIVE: &str =
    "This conversation is still settling an earlier turn. Your input was preserved.";

pub(super) fn available(store: &Database, session: i64, cx: &App) -> bool {
    cx.try_global::<Runs>()
        .is_none_or(|runs| !runs.0.contains_key(&SessionKey::new(store, session)))
}

pub(crate) fn running_view(
    store: &Database,
    session: i64,
    token: &TurnToken,
    cx: &App,
) -> Option<Entity<Conversation>> {
    let active = cx
        .try_global::<Runs>()?
        .0
        .get(&SessionKey::new(store, session))?;
    if &active.token == token {
        active.view.upgrade()
    } else {
        None
    }
}

pub(super) struct Admission {
    key: SessionKey,
    store: Database,
    view: WeakEntity<Conversation>,
    cancel: Arc<AtomicBool>,
}

pub(super) fn admit(
    store: &Database,
    session: i64,
    view: WeakEntity<Conversation>,
    cx: &mut App,
) -> Option<Admission> {
    use std::collections::hash_map::Entry;
    let key = SessionKey::new(store, session);
    let Entry::Vacant(slot) = cx.default_global::<Runs>().0.entry(key) else {
        return None;
    };
    let cancel = Arc::new(AtomicBool::new(false));
    slot.insert(Active {
        token: (&cancel).into(),
        view: view.clone(),
    });
    Some(Admission {
        key,
        store: store.clone(),
        view,
        cancel,
    })
}

impl Admission {
    pub(super) fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub(super) fn launch(
        self,
        input: runtime::TurnInput,
        remote: Option<Arc<Connections>>,
        projection: TurnProjection,
        cx: &mut App,
    ) -> std::sync::mpsc::Sender<Control> {
        let store = self.store.clone();
        let (controls, control_rx) = std::sync::mpsc::channel();
        let (tx, rx) = async_channel::bounded(8);
        let (worker_alive, done) = async_channel::bounded::<()>(1);
        let cancel = self.cancel_handle();
        cx.background_executor()
            .spawn(async move {
                // Keep admission through the worker's last save, including when
                // the recipient view has gone away or the worker unwinds.
                let _worker_alive = worker_alive;
                if let Some(client) = remote {
                    runtime::remote::run_with_client(client, input, control_rx, store, cancel, tx);
                } else {
                    runtime::run_turn(input, control_rx, store, cancel, tx);
                }
            })
            .detach();
        project(self.view.clone(), rx, projection, Some((self, done)), cx);
        controls
    }

    pub(super) fn release(self, cx: &mut App) -> bool {
        let runs = &mut cx.default_global::<Runs>().0;
        if runs
            .get(&self.key)
            .is_some_and(|run| run.token.matches(&self.cancel))
        {
            runs.remove(&self.key);
            true
        } else {
            false
        }
    }
}

pub(super) fn project(
    view: WeakEntity<Conversation>,
    rx: async_channel::Receiver<Update>,
    mut projection: TurnProjection,
    run: Option<(Admission, async_channel::Receiver<()>)>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        let terminal = loop {
            let Ok(update) = rx.recv().await else {
                break None;
            };
            if update.finished() {
                break Some(update);
            }
            if view
                .update(cx, |view, cx| projection.apply(view, update, cx))
                .is_err()
            {
                break None;
            }
            // A ready channel can refill without yielding to input or paint.
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1))
                .await;
        };
        drop(rx);
        let lease = if let Some((lease, done)) = run {
            let _ = done.recv().await;
            Some(lease)
        } else {
            None
        };
        cx.update(|cx| {
            if !lease.is_none_or(|lease| lease.release(cx)) {
                return;
            }
            let _ = view.update(cx, |view, cx| match terminal {
                Some(update) => projection.apply(view, update, cx),
                None => view.turn_updates_closed(
                    projection.generation,
                    projection.composer_origin.as_ref(),
                    cx,
                ),
            });
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext as _;

    #[gpui::test]
    fn a_detached_view_keeps_the_session_claimed_until_its_worker_exits(
        cx: &mut gpui::TestAppContext,
    ) {
        let store = Arc::new(std::sync::Mutex::new(
            vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let summary = runtime::locked(&store)
            .unwrap()
            .create("codex", "/tmp")
            .unwrap();
        let id = summary.id;
        let view = cx.new(|cx| Conversation::new(summary.clone(), store.clone(), cx));
        let (updates, rx) = async_channel::bounded(1);
        let (worker, done) = async_channel::bounded(1);
        let token = cx.update(|cx| {
            let admission = admit(&store, id, view.downgrade(), cx).unwrap();
            let token = (&admission.cancel).into();
            assert_eq!(running_view(&store, id, &token, cx), Some(view.clone()));
            assert!(admit(&store, id, view.downgrade(), cx).is_none());
            project(
                view.downgrade(),
                rx,
                TurnProjection::new(0, "codex".into(), String::new(), None, None),
                Some((admission, done)),
                cx,
            );
            token
        });
        drop(view);
        drop(updates);
        cx.run_until_parked();
        cx.update(|cx| assert!(!available(&store, id, cx)));
        drop(worker);
        cx.run_until_parked();
        let successor = cx.new(|cx| Conversation::new(summary, store.clone(), cx));
        cx.update(|cx| {
            let admission = admit(&store, id, successor.downgrade(), cx).unwrap();
            assert!(running_view(&store, id, &token, cx).is_none());
            let token = (&admission.cancel).into();
            assert_eq!(running_view(&store, id, &token, cx), Some(successor));
            assert!(admission.release(cx));
        });
    }
}
