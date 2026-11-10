//! Window checkout selection and its projection of the shared checkout catalog.
use gpui::{Context, Entity, EventEmitter, Subscription};
use std::{collections::HashMap, sync::Arc};
use vorkspace_core::scm::DiffIndex;

mod checkout;
pub(crate) use checkout::CheckoutCatalog;
use checkout::{Checkout, Snapshot};

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
    catalog: Entity<CheckoutCatalog>,
    checkout: Option<Entity<Checkout>>,
    observation: Option<Subscription>,
    snapshot: Option<Arc<Snapshot>>,
    source_revision: u64,
    /// Bumped on every observed change, for views that reread on it.
    pub epoch: u64,
    checkout_revision: u64,
    polling: bool,
}
impl EventEmitter<GitChanged> for ProjectGit {}

impl ProjectGit {
    #[cfg(test)]
    pub(crate) fn new(project: String, cx: &mut Context<Self>) -> Self {
        use gpui::AppContext as _;
        let catalog = cx.new(|_| CheckoutCatalog::default());
        Self::shared(project, catalog, cx)
    }

    pub(crate) fn shared(
        project: String,
        catalog: Entity<CheckoutCatalog>,
        cx: &mut Context<Self>,
    ) -> Self {
        let viewer = cx.entity_id();
        cx.on_release(move |this, cx| {
            if this.polling
                && let Some(checkout) = &this.checkout
            {
                checkout.update(cx, |checkout, cx| checkout.poll(viewer, false, cx));
            }
        })
        .detach();
        let mut this = Self {
            project: String::new(),
            catalog,
            checkout: None,
            observation: None,
            snapshot: None,
            source_revision: 0,
            epoch: 0,
            checkout_revision: 0,
            polling: false,
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
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.index.as_ref())
    }
    pub(crate) fn snapshot(&self) -> Option<Arc<DiffIndex>> {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.index.clone())
    }
    pub(crate) fn statuses(&self) -> Option<&StatusMap> {
        self.snapshot.as_ref().map(|snapshot| &snapshot.statuses)
    }
    pub(crate) fn available(&self) -> bool {
        !self.project.is_empty() && self.project != "~"
    }
    /// Publish the selected checkout's own state immediately, including when
    /// it is still loading. Peer windows keep their independent selections.
    pub(crate) fn set_project(&mut self, project: String, cx: &mut Context<Self>) {
        if project == self.project {
            return;
        }
        let viewer = cx.entity_id();
        if let Some(checkout) = self.checkout.take()
            && self.polling
        {
            checkout.update(cx, |checkout, cx| checkout.poll(viewer, false, cx));
        }
        self.observation = None;
        self.snapshot = None;
        self.source_revision = 0;
        self.project = project;
        self.checkout_revision += 1;
        self.epoch += 1;
        if self.available() {
            let checkout = self
                .catalog
                .update(cx, |catalog, cx| catalog.acquire(&self.project, cx));
            self.snapshot = checkout.read(cx).snapshot();
            self.source_revision = checkout.read(cx).revision;
            self.observation = Some(cx.observe(&checkout, |this, checkout, cx| {
                this.sync_checkout(&checkout, cx);
            }));
            checkout.update(cx, |checkout, cx| {
                checkout.attach(cx);
                if self.polling {
                    checkout.poll(viewer, true, cx);
                }
            });
            self.checkout = Some(checkout);
        }
        cx.emit(GitChanged);
        cx.notify();
    }

    fn sync_checkout(&mut self, source: &Entity<Checkout>, cx: &mut Context<Self>) {
        if self.checkout.as_ref() != Some(source) {
            return;
        }
        let checkout = source.read(cx);
        let snapshot = checkout.snapshot();
        let invalidate = self.source_revision != checkout.revision;
        self.source_revision = checkout.revision;
        let changed = self.snapshot.as_ref().map(Arc::as_ptr) != snapshot.as_ref().map(Arc::as_ptr);
        self.snapshot = snapshot;
        if invalidate {
            self.epoch += 1;
            cx.emit(GitChanged);
        }
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn set_polling(&mut self, polling: bool, cx: &mut Context<Self>) {
        if polling != self.polling {
            self.polling = polling;
            if let Some(checkout) = &self.checkout {
                let viewer = cx.entity_id();
                checkout.update(cx, |checkout, cx| checkout.poll(viewer, polling, cx));
            }
        }
    }

    pub(crate) fn resume(&mut self, cx: &mut Context<Self>) {
        if let Some(checkout) = &self.checkout {
            checkout.update(cx, |checkout, cx| {
                checkout.warm_choices(cx);
                checkout.scan(true, cx);
            });
        }
    }

    /// A local mutation invalidates every view of the same checkout.
    pub(crate) fn changed(&mut self, cx: &mut Context<Self>) {
        if let Some(checkout) = self.checkout.clone() {
            checkout.update(cx, Checkout::changed);
            self.sync_checkout(&checkout, cx);
        }
    }

    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(checkout) = &self.checkout {
            checkout.update(cx, |checkout, cx| checkout.scan(true, cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
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

    #[gpui::test]
    fn checkout_observation_is_shared_without_sharing_window_selection(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(dir.path().join("local.txt"), "untracked").unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let (first, second) = cx.update(|cx| {
            let catalog = cx.new(|_| CheckoutCatalog::default());
            let first = cx.new(|cx| ProjectGit::shared(path.clone(), catalog.clone(), cx));
            let second = cx.new(|cx| ProjectGit::shared(path.clone(), catalog, cx));
            assert_eq!(first.read(cx).checkout, second.read(cx).checkout);
            (first, second)
        });
        cx.run_until_parked();
        let peer_epoch = cx.update(|cx| {
            let snapshot = first.read(cx).snapshot().expect("checkout scanned");
            assert!(Arc::ptr_eq(&snapshot, &second.read(cx).snapshot().unwrap()));
            assert_eq!(snapshot.files.len(), 1);
            let epoch = second.read(cx).epoch;
            first.update(cx, ProjectGit::changed);
            epoch
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(second.read(cx).epoch > peer_epoch);
            second.update(cx, |git, cx| git.set_project("~".into(), cx));
            assert!(second.read(cx).snapshot().is_none());
            assert_eq!(first.read(cx).project(), path);
            let retained = first.read(cx).snapshot().unwrap();
            second.update(cx, |git, cx| git.set_project(path, cx));
            assert!(Arc::ptr_eq(&retained, &second.read(cx).snapshot().unwrap()));
        });
    }
}
