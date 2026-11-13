//! One lazily observed commit graph per checkout, shared across windows.
use super::*;
use vorkspace_model::git_graph::{self, GraphCommit, GraphRef, PathCommand, RowGraph};

const HISTORY_LIMIT: u32 = 200;
const HISTORY_ROW_HEIGHT: f32 = 44.;

pub(crate) struct HistoryItem {
    pub(crate) commit: scm::HistoryCommit,
    pub(crate) graph: RowGraph,
    pub(crate) badge: Option<GraphRef>,
}

#[derive(Default)]
pub(crate) struct HistoryState {
    pub(crate) items: Arc<[HistoryItem]>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(super) bytes: usize,
    readers: HashSet<EntityId>,
    last_read: Option<Instant>,
    reload_pending: bool,
}

impl Checkout {
    pub(crate) fn history(&self) -> &HistoryState {
        &self.history
    }

    pub(crate) fn watch_history(
        &mut self,
        viewer: EntityId,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if enabled {
            self.history.readers.insert(viewer);
            self.load_history(false, cx);
        } else {
            self.history.readers.remove(&viewer);
        }
    }

    pub(crate) fn load_history(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.history.readers.is_empty() {
            return;
        }
        if self.history.loading {
            self.history.reload_pending |= force;
            return;
        }
        if !force && self.history.last_read.is_some_and(|at| at.elapsed() < POLL) {
            return;
        }
        self.history.loading = true;
        self.history.error = None;
        let path = self.path.clone();
        let previous = self.history.items.clone();
        let read = cx.background_executor().spawn(async move {
            let commits = scm::history(Path::new(&path), Some(HISTORY_LIMIT))?.commits;
            let same = previous.len() == commits.len()
                && previous
                    .iter()
                    .zip(&commits)
                    .all(|(item, commit)| &item.commit == commit);
            Ok::<_, String>((!same).then(|| {
                let items = project(commits);
                let bytes = history_bytes(&items);
                (items, bytes)
            }))
        });
        cx.spawn(async move |checkout, cx| {
            let result = read.await;
            let _ = checkout.update(cx, |checkout, cx| {
                checkout.history.loading = false;
                if std::mem::take(&mut checkout.history.reload_pending) {
                    checkout.load_history(true, cx);
                } else {
                    checkout.history.last_read = Some(Instant::now());
                    match result {
                        Ok(Some((items, bytes))) => {
                            checkout.history.bytes = bytes;
                            checkout.history.items = items;
                            checkout.retain(cx);
                        }
                        Ok(None) => {}
                        Err(error) => checkout.history.error = Some(error),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn invalidate_history(&mut self, cx: &mut Context<Self>) {
        self.history.last_read = None;
        self.history.reload_pending |= self.history.loading;
        self.load_history(true, cx);
    }

    pub(super) fn refresh_history_if_due(&mut self, cx: &mut Context<Self>) {
        if self
            .history
            .last_read
            .is_none_or(|at| at.elapsed() >= FULL_REFRESH)
        {
            self.load_history(false, cx);
        }
    }
}

fn project(commits: Vec<scm::HistoryCommit>) -> Arc<[HistoryItem]> {
    let nodes: Vec<_> = commits
        .iter()
        .map(|commit| GraphCommit {
            sha: commit.sha.clone(),
            parents: commit.parents.clone(),
            head: commit.head,
            refs: commit
                .refs
                .iter()
                .map(|r| (r.name.clone(), r.kind.clone()))
                .collect(),
        })
        .collect();
    commits
        .into_iter()
        .zip(git_graph::layout(&nodes))
        .map(|(commit, row)| {
            let graph = git_graph::row_graph(&row, HISTORY_ROW_HEIGHT);
            let badge = row
                .refs
                .iter()
                .find(|r| r.color.is_some())
                .or(row.refs.first())
                .cloned();
            HistoryItem {
                commit,
                graph,
                badge,
            }
        })
        .collect()
}

fn history_bytes(items: &[HistoryItem]) -> usize {
    use std::mem::{size_of, size_of_val};
    size_of_val(items)
        + items
            .iter()
            .map(|item| {
                let commit = &item.commit;
                commit.sha.capacity()
                    + commit.short_sha.capacity()
                    + commit.author.capacity()
                    + commit.subject.capacity()
                    + commit.parents.capacity() * size_of::<String>()
                    + commit.parents.iter().map(String::capacity).sum::<usize>()
                    + commit.refs.capacity() * size_of::<scm::HistoryRef>()
                    + commit
                        .refs
                        .iter()
                        .map(|r| r.name.capacity() + r.kind.capacity())
                        .sum::<usize>()
                    + item
                        .badge
                        .as_ref()
                        .map_or(0, |r| r.name.capacity() + r.kind.capacity())
                    + item.graph.circles.capacity() * size_of::<git_graph::Circle>()
                    + item.graph.paths.capacity() * size_of::<git_graph::GraphPath>()
                    + item
                        .graph
                        .paths
                        .iter()
                        .map(|p| p.commands.capacity() * size_of::<PathCommand>())
                        .sum::<usize>()
            })
            .sum::<usize>()
}
