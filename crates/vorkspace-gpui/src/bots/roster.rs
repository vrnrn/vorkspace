//! Committed Bot state and ordered writes for every window on a store.
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Task};
use std::path::PathBuf;
use vorkspace_core::{bot::Roster, store::Store};
use vorkspace_runtime::runtime::{Database, locked};

pub(crate) struct MembershipChanged {
    pub(crate) ids: Vec<String>,
    pub(crate) vorkspace: Option<String>,
}

pub(crate) struct SharedRoster {
    pub(super) store: Database,
    pub(super) root: PathBuf,
    pub(super) snapshot: Result<Roster, String>,
    pub(super) revision: u64,
    writes: Option<Task<()>>,
}

impl EventEmitter<MembershipChanged> for SharedRoster {}

impl SharedRoster {
    pub(crate) fn install(store: Database, cx: &mut App) -> Entity<Self> {
        let (snapshot, root) = match locked(&store) {
            Ok(db) => (Roster::load(&db), vorkspace_core::store::bot_root(&db)),
            Err(error) => (
                Err(error),
                std::env::temp_dir().join(vorkspace_core::bot::new_id()),
            ),
        };
        let shared = cx.new(|_| Self {
            store,
            root,
            snapshot,
            revision: 0,
            writes: None,
        });
        let quit = shared.downgrade();
        cx.on_app_quit(move |cx| {
            let writes = quit
                .update(cx, |shared, _| shared.writes.take())
                .ok()
                .flatten();
            async move {
                if let Some(writes) = writes {
                    writes.await;
                }
            }
        })
        .detach();
        shared
    }

    pub(crate) fn member_ids(&self) -> Option<Vec<String>> {
        self.snapshot.as_ref().ok().map(Self::ids)
    }

    pub(crate) fn committed(&self) -> Option<&Roster> {
        self.snapshot.as_ref().ok()
    }

    fn ids(roster: &Roster) -> Vec<String> {
        roster
            .bots
            .iter()
            .filter(|bot| !bot.is_minion())
            .map(|bot| bot.id.clone())
            .collect()
    }

    /// Preserve command receipt ordering while publishing the roster even
    /// when the originating window closes or its later receipt write fails.
    pub(super) fn save<T: Send + 'static>(
        &mut self,
        roster: Roster,
        vorkspace: Option<String>,
        finalize: impl FnOnce(&Store) -> Result<T, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<Result<T, String>> {
        let store = self.store.clone();
        let previous = self.writes.take();
        let (send, receive) = async_channel::bounded(1);
        self.writes = Some(cx.spawn(async move |shared, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx.background_executor().spawn(async move {
                let db = locked(&store)?;
                let snapshot = roster.save_snapshot(&db)?;
                let result = finalize(&db);
                Ok::<_, String>((snapshot, result))
            });
            let result = match work.await {
                Ok((snapshot, result)) => {
                    let _ = shared.update(cx, |shared, cx| {
                        let ids = Self::ids(&snapshot);
                        let changed = shared.member_ids().as_ref() != Some(&ids);
                        shared.snapshot = Ok(snapshot);
                        shared.revision += 1;
                        if changed {
                            cx.emit(MembershipChanged { ids, vorkspace });
                        }
                        cx.notify();
                    });
                    result
                }
                Err(error) => Err(error),
            };
            let _ = send.try_send(result);
        }));
        receive
    }
}
