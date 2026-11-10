//! The active checkout's Git state, shared by the right panel and title bar.
use gpui::{Context, EventEmitter, Task};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use vorkspace_core::scm::{self, CheckoutStamp, DiffIndex, DiffStats};

const POLL: Duration = Duration::from_secs(2);
// The status probe covers changes to HEAD, the index and dirty worktree files.
// Reconcile other inputs (e.g. the default branch's remote ref) periodically.
const FULL_REFRESH: Duration = Duration::from_secs(20);
const RETAINED_CHECKOUTS: usize = 24;
const RETAINED_BYTES: usize = 16 * 1024 * 1024;

/// A disk or Git change: Changes, History and the Explorer reread.
pub(crate) struct GitChanged;

#[derive(Default, Clone, PartialEq)]
pub(crate) struct StatusMap {
    pub files: HashMap<String, String>,
    pub dirs: HashMap<String, String>,
}
fn priority(status: &str) -> u8 {
    match status {
        "modified" => 3,
        "deleted" => 2,
        "added" | "untracked" => 1,
        _ => 0,
    }
}
/// A file's change state, in the user's diff palette (default, colorblind
/// or high contrast) for added and deleted, and the theme's warning ink for
/// edits, all paired for light and dark.
pub(crate) fn status_color(status: &str) -> gpui::Hsla {
    match status {
        "untracked" => vorkspace_ui::theme::accent_color(),
        "added" => vorkspace_ui::theme::diff_foreground(true),
        "deleted" => vorkspace_ui::theme::diff_foreground(false),
        _ => vorkspace_ui::theme::warning(),
    }
}
/// useGitFileStatuses's `buildStatusMaps`: each folder takes its most
/// significant descendant status.
pub(crate) fn status_maps(index: &DiffIndex, root: &str) -> StatusMap {
    let mut map = StatusMap::default();
    for file in &index.files {
        map.files.insert(file.path.clone(), file.status.clone());
        let prio = priority(&file.status);
        if prio == 0 {
            continue;
        }
        let mut dir = parent(&file.path);
        while dir.len() > root.len() {
            let existing = map.dirs.get(dir).map_or(0, |s| priority(s));
            if prio <= existing {
                break;
            }
            map.dirs.insert(dir.to_owned(), file.status.clone());
            dir = parent(dir);
        }
    }
    map
}
fn parent(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

pub(crate) struct ProjectGit {
    project: String,
    index: Option<Arc<DiffIndex>>,
    pub stats: Option<DiffStats>,
    pub statuses: StatusMap,
    /// Bumped on every observed change, for views that reread on it.
    pub epoch: u64,
    versions: Option<HashMap<String, scm::FileDiffVersion>>,
    remembered: HashMap<String, RememberedCheckout>,
    clock: u64,
    checkout_revision: u64,
    generation: u64,
    in_flight: bool,
    pending_reload: bool,
    last_scan: Option<Instant>,
    stamp: Option<CheckoutStamp>,
    poll: Option<Task<()>>,
}
impl EventEmitter<GitChanged> for ProjectGit {}

struct GitRead {
    snapshot: scm::DiffSnapshot,
    stamp: Option<CheckoutStamp>,
}

struct RememberedCheckout {
    index: Arc<DiffIndex>,
    bytes: usize,
    touched: u64,
}

impl ProjectGit {
    pub(crate) fn new(project: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            project: String::new(),
            index: None,
            stats: None,
            statuses: StatusMap::default(),
            epoch: 0,
            versions: None,
            remembered: HashMap::new(),
            clock: 0,
            checkout_revision: 0,
            generation: 0,
            in_flight: false,
            pending_reload: false,
            last_scan: None,
            stamp: None,
            poll: None,
        };
        this.set_project(project, cx);
        this
    }
    pub(crate) fn project(&self) -> &str {
        &self.project
    }
    pub(crate) fn checkout_revision(&self) -> u64 {
        self.checkout_revision
    }
    pub(crate) fn index(&self) -> Option<&DiffIndex> {
        self.index.as_deref()
    }
    pub(crate) fn snapshot(&self) -> Option<Arc<DiffIndex>> {
        self.index.clone()
    }
    pub(crate) fn available(&self) -> bool {
        !self.project.is_empty() && self.project != "~"
    }
    /// The first frame of a switch already shows the new checkout's last
    /// snapshot, never the previous one's files and actions.
    pub(crate) fn set_project(&mut self, project: String, cx: &mut Context<Self>) {
        if project == self.project {
            return;
        }
        self.project = project;
        self.checkout_revision += 1;
        self.generation += 1;
        self.clock += 1;
        self.index = self.remembered.get_mut(&self.project).map(|entry| {
            entry.touched = self.clock;
            entry.index.clone()
        });
        self.stats = self.index.as_deref().map(stats_of);
        self.statuses = self
            .index
            .as_ref()
            .map(|i| status_maps(i, &self.project))
            .unwrap_or_default();
        self.last_scan = None;
        self.stamp = None;
        self.versions = None;
        self.epoch += 1;
        cx.emit(GitChanged);
        cx.notify();
        self.warm_choices(cx);
        self.reload(cx);
    }
    pub(crate) fn set_polling(&mut self, polling: bool, cx: &mut Context<Self>) {
        if polling == self.poll.is_some() {
            return;
        }
        if !polling {
            self.poll = None;
            return;
        }
        self.request_scan(false, cx);
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this
                    .update(cx, |this, cx| this.request_scan(false, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    pub(crate) fn resume(&mut self, cx: &mut Context<Self>) {
        self.warm_choices(cx);
        self.reload(cx);
    }
    /// A mutation or file operation: reread now and tell every view.
    pub(crate) fn changed(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.epoch += 1;
        cx.emit(GitChanged);
        self.warm_choices(cx);
        self.reload(cx);
    }
    /// Project selection warms both menus before the user opens either one.
    fn warm_choices(&self, cx: &Context<Self>) {
        if !self.available() {
            return;
        }
        let project = self.project.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = vorkspace_model::git_cache::load(std::path::Path::new(&project));
            })
            .detach();
    }
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.request_scan(true, cx);
    }

    fn request_scan(&mut self, force: bool, cx: &mut Context<Self>) {
        if !self.available() {
            return;
        }
        if self.in_flight {
            self.pending_reload |= force;
            return;
        }
        self.in_flight = true;
        let generation = self.generation;
        let path = PathBuf::from(&self.project);
        let known = self
            .last_scan
            .filter(|at| !force && at.elapsed() < FULL_REFRESH)
            .and(self.stamp);
        let work = cx.background_executor().spawn(async move {
            let stamp = CheckoutStamp::read(&path);
            if known.is_some() && known == stamp {
                return Ok(None);
            }
            let snapshot = scm::DiffSnapshot::read(&path)?;
            Ok::<_, String>(Some(GitRead { snapshot, stamp }))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.in_flight = false;
                if this.generation == generation
                    && let Ok(Some(read)) = result
                {
                    this.publish(read, cx);
                }
                if std::mem::take(&mut this.pending_reload) {
                    this.reload(cx);
                }
            });
        })
        .detach();
    }
    fn publish(&mut self, read: GitRead, cx: &mut Context<Self>) {
        let GitRead { snapshot, stamp } = read;
        let scm::DiffSnapshot { index, versions } = snapshot;
        self.last_scan = Some(Instant::now());
        self.stamp = stamp.filter(|_| versions.is_some());
        let changed = self.index.as_deref() != Some(&index);
        let had_snapshot = self.index.is_some();
        if changed {
            self.stats = Some(stats_of(&index));
            self.statuses = status_maps(&index, &self.project);
            let index = Arc::new(index);
            self.remember(index.clone());
            self.index = Some(index);
        }
        if (had_snapshot && changed) || self.versions != versions {
            self.versions = versions;
            self.epoch += 1;
            cx.emit(GitChanged);
        }
        if changed {
            cx.notify();
        }
    }

    fn remember(&mut self, index: Arc<DiffIndex>) {
        let bytes = index.files.capacity() * std::mem::size_of::<scm::ChangedFile>()
            + index
                .files
                .iter()
                .map(|file| {
                    file.path.capacity() + file.relative.capacity() + file.status.capacity()
                })
                .sum::<usize>();
        if bytes > RETAINED_BYTES {
            self.remembered.remove(&self.project);
            return;
        }
        self.clock += 1;
        self.remembered.insert(
            self.project.clone(),
            RememberedCheckout {
                index,
                bytes,
                touched: self.clock,
            },
        );
        let mut bytes: usize = self.remembered.values().map(|entry| entry.bytes).sum();
        while self.remembered.len() > RETAINED_CHECKOUTS || bytes > RETAINED_BYTES {
            let Some(oldest) = self
                .remembered
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(gone) = self.remembered.remove(&oldest) {
                bytes -= gone.bytes;
            }
        }
    }
    pub(crate) fn has_changes(&self) -> bool {
        self.stats.as_ref().is_some_and(|s| s.files > 0)
    }
}
fn stats_of(index: &DiffIndex) -> DiffStats {
    DiffStats {
        files: index.files.len() as i64,
        additions: index.additions,
        deletions: index.deletions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vorkspace_core::scm::ChangedFile;
    fn file(path: &str, status: &str) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            relative: path.trim_start_matches("/r/").into(),
            status: status.into(),
            additions: 0,
            deletions: 0,
            staged: false,
            unstaged: true,
        }
    }
    #[test]
    fn folders_take_their_most_significant_status() {
        let index = DiffIndex {
            files: vec![
                file("/r/a/b/new.rs", "untracked"),
                file("/r/a/b/old.rs", "modified"),
                file("/r/a/gone.rs", "deleted"),
            ],
            ..Default::default()
        };
        let map = status_maps(&index, "/r");
        assert_eq!(map.dirs["/r/a/b"], "modified");
        assert_eq!(map.dirs["/r/a"], "modified");
        assert!(!map.dirs.contains_key("/r"));
        assert_eq!(map.files["/r/a/gone.rs"], "deleted");
    }
}
