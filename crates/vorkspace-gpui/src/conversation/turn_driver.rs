//! Optional view projection of turns whose admission belongs to the runtime.
use super::{Conversation, turn_projection::TurnProjection};
use gpui::{App, Entity, Global, WeakEntity};
use runtime::turns::{self, SessionKey};
use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
};
use vorkspace_core::{agent::Control, process::TurnToken};
use vorkspace_runtime::runtime::{self, Database, Update};

struct Active {
    token: TurnToken,
    view: WeakEntity<Conversation>,
}

#[derive(Default)]
struct RunViews {
    runtime: turns::Runs,
    views: HashMap<SessionKey, Active>,
}
impl Global for RunViews {}

pub(super) const ALREADY_ACTIVE: &str =
    "This conversation is still settling an earlier turn. Your input was preserved.";

pub(super) fn available(store: &Database, session: i64, cx: &App) -> bool {
    cx.try_global::<RunViews>()
        .is_none_or(|runs| runs.runtime.available(store, session))
}

pub(crate) fn running_view(
    store: &Database,
    session: i64,
    token: &TurnToken,
    cx: &App,
) -> Option<Entity<Conversation>> {
    let runs = cx.try_global::<RunViews>()?;
    let active = runs.views.get(&SessionKey::new(store, session))?;
    if &active.token == token && runs.runtime.owns(store, session, token) {
        active.view.upgrade()
    } else {
        None
    }
}

pub(super) struct Admission {
    worker: turns::Admission,
    view: WeakEntity<Conversation>,
}

pub(super) struct ProjectionOwner {
    key: SessionKey,
    token: TurnToken,
}

pub(crate) fn stop_running(
    store: &Database,
    session: i64,
    token: &TurnToken,
    cx: &mut App,
) -> bool {
    if let Some(view) = running_view(store, session, token, cx)
        && view.update(cx, |view, cx| view.stop_running_turn(token, cx))
    {
        return true;
    }
    cx.try_global::<RunViews>()
        .is_some_and(|runs| runs.runtime.stop(store, session, token))
}

pub(super) fn admit(
    store: &Database,
    session: i64,
    view: WeakEntity<Conversation>,
    cx: &mut App,
) -> Option<Admission> {
    let runs = cx.default_global::<RunViews>();
    let worker = runs.runtime.admit(store, session)?;
    runs.views.insert(
        worker.key(),
        Active {
            token: worker.token(),
            view: view.clone(),
        },
    );
    Some(Admission { worker, view })
}

impl Admission {
    pub(super) fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.worker.cancel_handle()
    }

    pub(super) fn into_worker(self) -> (turns::Admission, ProjectionOwner) {
        let owner = ProjectionOwner {
            key: self.worker.key(),
            token: self.worker.token(),
        };
        (self.worker, owner)
    }

    pub(super) fn launch(
        self,
        input: runtime::TurnInput,
        projection: TurnProjection,
        cx: &mut App,
    ) -> std::sync::mpsc::Sender<Control> {
        let view = self.view.clone();
        let (worker, projection_owner) = self.into_worker();
        let (controls, control_rx) = std::sync::mpsc::channel();
        let (tx, rx) = async_channel::bounded(8);
        let (worker_alive, done) = async_channel::bounded::<()>(1);
        cx.background_executor()
            .spawn(async move {
                let _worker_alive = worker_alive;
                worker.run(input, control_rx, tx);
            })
            .detach();
        project(view, rx, projection, Some((projection_owner, done)), cx);
        controls
    }
}

impl ProjectionOwner {
    pub(super) fn release(self, cx: &mut App) -> bool {
        let runs = &mut cx.default_global::<RunViews>().views;
        if runs
            .get(&self.key)
            .is_some_and(|run| run.token == self.token)
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
    run: Option<(ProjectionOwner, async_channel::Receiver<()>)>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        let mut attached = true;
        let terminal = loop {
            let Ok(update) = rx.recv().await else {
                break None;
            };
            if update.finished() {
                break Some(update);
            }
            if attached
                && view
                    .update(cx, |view, cx| projection.apply(view, update, cx))
                    .is_err()
            {
                attached = false;
                if run.is_none() {
                    break None;
                }
            }
            if attached {
                // A ready channel can refill without yielding to input or paint.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1))
                    .await;
            }
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
            vorkspace_core::store::Store::test(std::path::Path::new(":memory:")).unwrap(),
        ));
        let summary = runtime::locked(&store)
            .unwrap()
            .create("codex", "/tmp")
            .unwrap();
        let id = summary.id;
        let view = cx.new(|cx| Conversation::new(summary.clone(), store.clone(), cx));
        let (updates, rx) = async_channel::bounded(1);
        let (alive, done) = async_channel::bounded(1);
        let (worker, token) = cx.update(|cx| {
            let admission = admit(&store, id, view.downgrade(), cx).unwrap();
            let token = admission.worker.token();
            assert_eq!(running_view(&store, id, &token, cx), Some(view.clone()));
            assert!(admit(&store, id, view.downgrade(), cx).is_none());
            let (worker, owner) = admission.into_worker();
            project(
                view.downgrade(),
                rx,
                TurnProjection::new(0, "codex".into(), String::new(), None, None),
                Some((owner, done)),
                cx,
            );
            (worker, token)
        });
        drop(view);
        updates.try_send(Update::default()).unwrap();
        cx.run_until_parked();
        assert!(
            !updates.is_closed(),
            "the admitted worker keeps its progress receiver"
        );
        cx.update(|cx| {
            assert!(!available(&store, id, cx));
            assert!(stop_running(&store, id, &token, cx));
        });
        assert!(
            worker
                .cancel_handle()
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        drop(updates);
        cx.run_until_parked();
        cx.update(|cx| assert!(!available(&store, id, cx)));
        drop(worker);
        let successor = cx.new(|cx| Conversation::new(summary, store.clone(), cx));
        let (worker, owner, next) = cx.update(|cx| {
            let admission = admit(&store, id, successor.downgrade(), cx).unwrap();
            assert!(running_view(&store, id, &token, cx).is_none());
            let next = admission.worker.token();
            let (worker, owner) = admission.into_worker();
            (worker, owner, next)
        });
        // The earlier projection cleanup reaches the UI after its successor
        // registered. It cannot remove that successor's view or cancellation.
        drop(alive);
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(running_view(&store, id, &next, cx), Some(successor));
            assert!(!stop_running(&store, id, &token, cx));
            assert!(!available(&store, id, cx));
            assert!(owner.release(cx));
        });
        assert!(
            !worker
                .cancel_handle()
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        drop(worker);
        cx.update(|cx| assert!(available(&store, id, cx)));
    }

    #[gpui::test]
    fn a_detached_observer_retires_without_retaining_a_runtime_run(cx: &mut gpui::TestAppContext) {
        let store = Arc::new(std::sync::Mutex::new(
            vorkspace_core::store::Store::test(std::path::Path::new(":memory:")).unwrap(),
        ));
        let summary = runtime::locked(&store)
            .unwrap()
            .create("codex", "/tmp")
            .unwrap();
        let id = summary.id;
        let view = cx.new(|cx| Conversation::new(summary, store.clone(), cx));
        let (updates, rx) = async_channel::bounded(1);
        cx.update(|cx| {
            project(
                view.downgrade(),
                rx,
                TurnProjection::new(0, "codex".into(), String::new(), None, None),
                None,
                cx,
            )
        });
        drop(view);
        updates.try_send(Update::default()).unwrap();
        cx.run_until_parked();
        assert!(updates.is_closed());
        cx.update(|cx| assert!(available(&store, id, cx)));
    }
}
