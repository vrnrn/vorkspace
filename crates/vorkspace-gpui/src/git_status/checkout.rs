//! A checkout owns one scan and one immutable observation for all its windows.
use super::{StatusMap, status_maps};
use gpui::{AppContext, Context, Entity, EntityId, Task, WeakEntity};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use vorkspace_core::{
    files::path_key,
    scm::{self, CheckoutStamp, DiffIndex},
};

mod history;
#[cfg(target_os = "macos")]
mod watch;
pub(crate) use history::HistoryItem;
use history::HistoryState;

const POLL: Duration = Duration::from_secs(2);
const FULL_REFRESH: Duration = Duration::from_secs(20);
const RETAINED_CHECKOUTS: usize = 24;
const RETAINED_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Refresh {
    Probe,
    Reconcile,
}

#[derive(Default)]
pub(crate) struct CheckoutCatalog {
    checkouts: HashMap<String, WeakEntity<Checkout>>,
    retained: HashMap<String, Retained>,
    clock: u64,
}

struct Retained {
    _checkout: Entity<Checkout>,
    bytes: usize,
    touched: u64,
}

impl CheckoutCatalog {
    pub(super) fn acquire(&mut self, path: &str, cx: &mut Context<Self>) -> Entity<Checkout> {
        let key = path_key(path);
        if let Some(checkout) = self.checkouts.get(&key).and_then(WeakEntity::upgrade) {
            if let Some(entry) = self.retained.get_mut(&key) {
                self.clock += 1;
                entry.touched = self.clock;
            }
            return checkout;
        }
        self.checkouts
            .retain(|_, checkout| checkout.upgrade().is_some());
        let catalog = cx.weak_entity();
        let checkout = cx.new(|_| Checkout::new(path.to_owned(), catalog));
        self.checkouts.insert(key, checkout.downgrade());
        checkout
    }

    fn retain(&mut self, path: &str, checkout: Entity<Checkout>, bytes: usize) {
        let key = path_key(path);
        if bytes > RETAINED_BYTES {
            self.retained.remove(&key);
            return;
        }
        self.clock += 1;
        self.retained.insert(
            key,
            Retained {
                _checkout: checkout,
                bytes,
                touched: self.clock,
            },
        );
        let mut bytes: usize = self.retained.values().map(|entry| entry.bytes).sum();
        while self.retained.len() > RETAINED_CHECKOUTS || bytes > RETAINED_BYTES {
            let oldest = self
                .retained
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(path, _)| path.clone())
                .expect("checkout budget exceeded");
            bytes -= self
                .retained
                .remove(&oldest)
                .expect("retained checkout")
                .bytes;
        }
    }
}

pub(super) struct Snapshot {
    pub(super) index: Arc<DiffIndex>,
    pub(super) statuses: StatusMap,
    pub(super) local_head: Option<scm::LocalHead>,
    bytes: usize,
}

impl Snapshot {
    fn new(index: DiffIndex, local_head: Option<scm::LocalHead>, path: &str) -> Self {
        let statuses = status_maps(&index, path);
        // Estimates include the shared index and both derived status maps.
        let bytes = index.files.capacity() * std::mem::size_of::<scm::ChangedFile>()
            + index
                .files
                .iter()
                .map(|file| {
                    file.path.capacity() + file.relative.capacity() + file.status.capacity()
                })
                .sum::<usize>()
            + [&statuses.files, &statuses.dirs]
                .into_iter()
                .map(|map| {
                    map.capacity() * std::mem::size_of::<(String, String)>()
                        + map
                            .iter()
                            .map(|(path, status)| path.capacity() + status.capacity())
                            .sum::<usize>()
                })
                .sum::<usize>();
        Self {
            statuses,
            index: Arc::new(index),
            local_head,
            bytes,
        }
    }
}

type Versions = HashMap<String, scm::FileDiffVersion>;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct EditorBaselineRevision {
    pub(crate) staged: bool,
    head: Option<String>,
    version: Option<scm::FileDiffVersion>,
}

#[derive(Clone)]
struct Observation {
    snapshot: Arc<Snapshot>,
    versions: Option<Arc<Versions>>,
    stamp: Option<CheckoutStamp>,
}

impl Observation {
    fn read(
        path: &str,
        known_stamp: Option<CheckoutStamp>,
        before: Option<Self>,
    ) -> Result<Option<Self>, String> {
        let cwd = Path::new(path);
        let Some(scm::DiffSnapshot {
            index,
            versions,
            stamp,
            local_head,
        }) = scm::DiffSnapshot::observe(cwd, known_stamp)?
        else {
            return Ok(None);
        };
        let snapshot = before
            .as_ref()
            .map(|read| &read.snapshot)
            .filter(|known| *known.index == index && known.local_head == local_head)
            .cloned()
            .unwrap_or_else(|| Arc::new(Snapshot::new(index, local_head, path)));
        let versions = versions.map(|versions| {
            before
                .and_then(|read| read.versions)
                .filter(|known| **known == versions)
                .unwrap_or_else(|| Arc::new(versions))
        });
        Ok(Some(Self {
            snapshot,
            stamp: stamp.filter(|_| versions.is_some()),
            versions,
        }))
    }

    fn bytes(&self) -> usize {
        self.snapshot.bytes
            + self.versions.as_ref().map_or(0, |versions| {
                versions.capacity() * std::mem::size_of::<(String, scm::FileDiffVersion)>()
                    + versions
                        .keys()
                        .map(|key| key.capacity() + 128)
                        .sum::<usize>()
            })
    }
}

pub(crate) struct Checkout {
    path: String,
    catalog: WeakEntity<CheckoutCatalog>,
    observation: Option<Observation>,
    history: HistoryState,
    pub(super) revision: u64,
    generation: u64,
    scanning: bool,
    refresh_pending: Option<Refresh>,
    last_scan: Option<Instant>,
    pollers: HashSet<EntityId>,
    poll: Option<Task<()>>,
}

impl Checkout {
    fn new(path: String, catalog: WeakEntity<CheckoutCatalog>) -> Self {
        Self {
            path,
            catalog,
            observation: None,
            history: HistoryState::default(),
            revision: 0,
            generation: 0,
            scanning: false,
            refresh_pending: None,
            last_scan: None,
            pollers: Default::default(),
            poll: None,
        }
    }

    pub(super) fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.observation.as_ref().map(|read| read.snapshot.clone())
    }

    pub(crate) fn editor_baseline_revision(
        &self,
        relative: &str,
    ) -> Option<EditorBaselineRevision> {
        let read = self.observation.as_ref()?;
        let versions = read.versions.as_ref()?;
        let index = &read.snapshot.index;
        let staged = index
            .files
            .iter()
            .find(|file| file.relative == relative)
            .is_some_and(|file| file.staged && !file.unstaged);
        let side = if staged { "staged" } else { "unstaged" };
        Some(EditorBaselineRevision {
            staged,
            head: index.head.clone(),
            version: versions.get(&format!("{side}:{relative}")).cloned(),
        })
    }

    pub(super) fn attach(&mut self, cx: &mut Context<Self>) {
        if self.last_scan.is_some_and(|at| at.elapsed() < POLL) {
            return;
        }
        self.scan(Refresh::Probe, cx);
    }

    pub(crate) fn poll(&mut self, viewer: EntityId, enabled: bool, cx: &mut Context<Self>) {
        if enabled {
            self.pollers.insert(viewer);
        } else {
            self.pollers.remove(&viewer);
        }
        if self.pollers.is_empty() {
            self.poll = None;
        } else if self.poll.is_none() {
            self.attach(cx);
            #[cfg(target_os = "macos")]
            let watch_path = (!cx.is_test()).then(|| self.path.clone());
            self.poll = Some(cx.spawn(async move |this, cx| {
                #[cfg(target_os = "macos")]
                if let Some(path) = watch_path {
                    watch::observe(&this, path, cx).await;
                }
                loop {
                    cx.background_executor().timer(POLL).await;
                    if this
                        .update(cx, |this, cx| {
                            if !this.scanning {
                                this.scan(Refresh::Probe, cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
    }

    pub(crate) fn changed(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.revision += 1;
        cx.notify();
        self.reconcile(cx);
    }

    pub(super) fn reconcile(&mut self, cx: &mut Context<Self>) {
        self.scan(Refresh::Reconcile, cx);
    }

    fn scan(&mut self, refresh: Refresh, cx: &mut Context<Self>) {
        if self.scanning {
            self.refresh_pending = Some(
                self.refresh_pending
                    .map_or(refresh, |pending| pending.max(refresh)),
            );
            return;
        }
        self.scanning = true;
        let generation = self.generation;
        let path = self.path.clone();
        let before = self.observation.clone();
        let stamp = self
            .last_scan
            .filter(|at| refresh == Refresh::Probe && at.elapsed() < FULL_REFRESH)
            .and_then(|_| before.as_ref()?.stamp);
        let work = cx
            .background_executor()
            .spawn(async move { Observation::read(&path, stamp, before) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.scanning = false;
                if this.generation == generation
                    && let Ok(Some(read)) = result
                {
                    this.publish(read, cx);
                }
                this.refresh_history_if_due(cx);
                if let Some(refresh) = this.refresh_pending.take() {
                    this.scan(refresh, cx);
                }
            });
        })
        .detach();
    }

    fn publish(&mut self, read: Observation, cx: &mut Context<Self>) {
        self.last_scan = Some(Instant::now());
        let changed = self
            .observation
            .as_ref()
            .is_none_or(|before| !Arc::ptr_eq(&before.snapshot, &read.snapshot));
        let invalidate = self
            .observation
            .as_ref()
            .map_or(read.versions.is_some(), |before| {
                changed
                    || before.versions.as_ref().map(Arc::as_ptr)
                        != read.versions.as_ref().map(Arc::as_ptr)
            });
        let history_changed = self.observation.as_ref().is_some_and(|before| {
            history_inputs_changed(&before.snapshot.index, &read.snapshot.index)
                || before.snapshot.local_head != read.snapshot.local_head
        });
        self.observation = Some(read);
        if invalidate {
            self.revision += 1;
        }
        if history_changed {
            self.invalidate_history(cx);
        }
        if changed || invalidate {
            self.retain(cx);
            cx.notify();
        }
    }

    fn retain(&self, cx: &mut Context<Self>) {
        let bytes = self.observation.as_ref().map_or(0, Observation::bytes) + self.history.bytes;
        let checkout = cx.entity();
        let _ = self
            .catalog
            .update(cx, |catalog, _| catalog.retain(&self.path, checkout, bytes));
    }
}

fn history_inputs_changed(before: &DiffIndex, after: &DiffIndex) -> bool {
    before.head != after.head
        || before.branch != after.branch
        || before.upstream != after.upstream
        || before.default_branch != after.default_branch
        || before.ahead != after.ahead
        || before.behind != after.behind
        || before.ahead_of_default != after.ahead_of_default
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_tree_edits_do_not_invalidate_the_commit_graph() {
        let before = DiffIndex {
            head: Some("old-tip".into()),
            branch: Some("main".into()),
            ..Default::default()
        };
        let mut after = DiffIndex {
            additions: 9,
            deletions: 2,
            ..before.clone()
        };
        assert!(!history_inputs_changed(&before, &after));
        after.head = Some("new-tip".into());
        assert!(history_inputs_changed(&before, &after));
        after.head.clone_from(&before.head);
        after.branch = Some("release".into());
        assert!(history_inputs_changed(&before, &after));
    }
}
