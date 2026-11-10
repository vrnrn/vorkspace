//! A checkout owns one scan and one immutable observation for all its windows.
use super::{StatusMap, status_maps};
use gpui::{AppContext, Context, Entity, EntityId, Task, WeakEntity};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use vorkspace_core::scm::{self, CheckoutStamp, DiffIndex};

const POLL: Duration = Duration::from_secs(2);
const FULL_REFRESH: Duration = Duration::from_secs(20);
const RETAINED_CHECKOUTS: usize = 24;
const RETAINED_BYTES: usize = 16 * 1024 * 1024;

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
        if let Some(checkout) = self.checkouts.get(path).and_then(WeakEntity::upgrade) {
            if let Some(entry) = self.retained.get_mut(path) {
                self.clock += 1;
                entry.touched = self.clock;
            }
            return checkout;
        }
        self.checkouts
            .retain(|_, checkout| checkout.upgrade().is_some());
        let catalog = cx.weak_entity();
        let checkout = cx.new(|_| Checkout::new(path.to_owned(), catalog));
        self.checkouts.insert(path.to_owned(), checkout.downgrade());
        checkout
    }

    fn retain(&mut self, path: &str, checkout: Entity<Checkout>, bytes: usize) {
        if bytes > RETAINED_BYTES {
            self.retained.remove(path);
            return;
        }
        self.clock += 1;
        self.retained.insert(
            path.to_owned(),
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
    bytes: usize,
}

impl Snapshot {
    fn new(index: DiffIndex, path: &str) -> Self {
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
            bytes,
        }
    }
}

type Versions = HashMap<String, scm::FileDiffVersion>;

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
        let stamp = CheckoutStamp::read(cwd);
        if known_stamp.is_some() && known_stamp == stamp {
            return Ok(None);
        }
        let scm::DiffSnapshot { index, versions } = scm::DiffSnapshot::read(cwd)?;
        let snapshot = before
            .as_ref()
            .map(|read| &read.snapshot)
            .filter(|known| *known.index == index)
            .cloned()
            .unwrap_or_else(|| Arc::new(Snapshot::new(index, path)));
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

pub(super) struct Checkout {
    path: String,
    catalog: WeakEntity<CheckoutCatalog>,
    observation: Option<Observation>,
    pub(super) revision: u64,
    generation: u64,
    scanning: bool,
    reload_pending: bool,
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
            revision: 0,
            generation: 0,
            scanning: false,
            reload_pending: false,
            last_scan: None,
            pollers: Default::default(),
            poll: None,
        }
    }

    pub(super) fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.observation.as_ref().map(|read| read.snapshot.clone())
    }

    pub(super) fn attach(&mut self, cx: &mut Context<Self>) {
        if self.last_scan.is_some_and(|at| at.elapsed() < POLL) {
            return;
        }
        if self.observation.is_none() && !self.scanning {
            self.warm_choices(cx);
        }
        self.scan(false, cx);
    }

    pub(super) fn poll(&mut self, viewer: EntityId, enabled: bool, cx: &mut Context<Self>) {
        if enabled {
            self.pollers.insert(viewer);
        } else {
            self.pollers.remove(&viewer);
        }
        if self.pollers.is_empty() {
            self.poll = None;
        } else if self.poll.is_none() {
            self.attach(cx);
            self.poll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL).await;
                    if this.update(cx, |this, cx| this.scan(false, cx)).is_err() {
                        break;
                    }
                }
            }));
        }
    }

    pub(super) fn changed(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.revision += 1;
        cx.notify();
        self.warm_choices(cx);
        self.scan(true, cx);
    }

    pub(super) fn warm_choices(&self, cx: &Context<Self>) {
        let path = self.path.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = vorkspace_model::git_cache::load(Path::new(&path));
            })
            .detach();
    }

    pub(super) fn scan(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.scanning {
            self.reload_pending |= force;
            return;
        }
        self.scanning = true;
        let generation = self.generation;
        let path = self.path.clone();
        let before = self.observation.clone();
        let stamp = self
            .last_scan
            .filter(|at| !force && at.elapsed() < FULL_REFRESH)
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
                if std::mem::take(&mut this.reload_pending) {
                    this.scan(true, cx);
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
        let bytes = (changed || invalidate).then(|| read.bytes());
        self.observation = Some(read);
        if invalidate {
            self.revision += 1;
        }
        if let Some(bytes) = bytes {
            let checkout = cx.entity();
            let _ = self
                .catalog
                .update(cx, |catalog, _| catalog.retain(&self.path, checkout, bytes));
            cx.notify();
        }
    }
}
