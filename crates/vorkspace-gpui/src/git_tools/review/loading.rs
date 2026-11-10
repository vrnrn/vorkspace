//! Loads Review sources and publishes complete groups of diff updates.
use super::{Loaded, ReviewPane, ReviewTab, Source, entries};
use crate::git_tools::diff_view::DiffFile;
use gpui::{Context, SharedString};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use vorkspace_core::{
    diff,
    scm::{self, ChangedFile, FileDiff, FileDiffVersion},
};

const CONCURRENCY: usize = 4;
const PUBLISH_INTERVAL: Duration = Duration::from_millis(16);

enum FileReader {
    Checkout(scm::WorktreeReview),
    Commit(scm::CommitReview),
    Session { cwd: String, id: String },
}

impl FileReader {
    fn file(&self, relative: &str, staged: Option<bool>) -> Result<FileDiff, String> {
        match self {
            Self::Checkout(reader) => reader.file(relative, staged == Some(true)),
            Self::Commit(reader) => reader.file(relative),
            Self::Session { cwd, id } => {
                let store =
                    vorkspace_core::checkpoint::store().ok_or("Session changes are unavailable")?;
                store.exclusive(|s| s.file_diff(id, cwd, relative))
            }
        }
    }
}

struct Listed {
    files: Vec<ChangedFile>,
    versions: Option<HashMap<String, FileDiffVersion>>,
    reader: FileReader,
}

fn session_files(cwd: &str, session: i64) -> Result<Listed, String> {
    let store = vorkspace_core::checkpoint::store().ok_or("Session changes are unavailable")?;
    let id = session.to_string();
    let status = store.exclusive(|s| s.status(&id, cwd))?;
    let files = status
        .files
        .into_iter()
        .map(|f| ChangedFile {
            path: f.path,
            relative: f.relative,
            status: f.status,
            additions: f.additions,
            deletions: f.deletions,
            staged: false,
            unstaged: true,
        })
        .collect();
    Ok(Listed {
        files,
        versions: None,
        reader: FileReader::Session {
            cwd: cwd.to_owned(),
            id,
        },
    })
}

impl ReviewPane {
    /// Focus and Git changes reread the working tree; commits never change.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.visible
            && let Some(source) = self.sources.get(self.state.active)
            && matches!(
                source.tab,
                ReviewTab::Changes { .. } | ReviewTab::Session { .. }
            )
        {
            self.load_active(cx);
        }
    }

    pub(super) fn load_active(&mut self, cx: &mut Context<Self>) {
        if !self.visible || self.deferred {
            return;
        }
        for (ix, source) in self.sources.iter_mut().enumerate() {
            if ix != self.state.active && source.load.is_some() {
                source.cancel();
            }
            if let Some(editor) = &source.editor {
                editor.update(cx, |e, cx| {
                    e.set_visible(self.visible && ix == self.state.active, cx)
                });
            }
        }
        let ix = self.state.active;
        let Some(source) = self.sources.get_mut(ix) else {
            return;
        };
        if source.editor.is_some() {
            return;
        }
        if source.complete && matches!(source.tab, ReviewTab::Commit { .. }) {
            return;
        }
        if source.load.is_some() {
            source.refresh_pending = true;
            return;
        }
        let generation = source.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let tab = source.tab.clone();
        let counter = source.generation.clone();
        let known_index = match &tab {
            ReviewTab::Changes { cwd, .. } if self.git.read(cx).project() == cwd => {
                self.git.read(cx).snapshot()
            }
            _ => None,
        };
        let list = cx.background_executor().spawn({
            let tab = tab.clone();
            async move {
                match &tab {
                    ReviewTab::File { .. } => unreachable!(),
                    ReviewTab::Changes { cwd, .. } => {
                        let root = PathBuf::from(cwd);
                        let reader = scm::WorktreeReview::open(&root)?;
                        let scm::DiffSnapshot { index, versions } =
                            reader.snapshot(known_index.as_deref())?;
                        Ok(Listed {
                            files: index.files,
                            versions,
                            reader: FileReader::Checkout(reader),
                        })
                    }
                    ReviewTab::Commit { cwd, sha, .. } => {
                        let reader = scm::CommitReview::open(&PathBuf::from(cwd), sha)?;
                        Ok(Listed {
                            files: reader.files()?,
                            versions: None,
                            reader: FileReader::Commit(reader),
                        })
                    }
                    ReviewTab::Session { cwd, session, .. } => session_files(cwd, *session),
                }
            }
        });
        let task = cx.spawn(async move |this, cx| {
            let listed = list.await;
            let order = this.update(cx, |this, cx| {
                let source = this
                    .sources
                    .iter_mut()
                    .find(|s| Arc::ptr_eq(&s.generation, &counter))?;
                if counter.load(Ordering::SeqCst) != generation {
                    return None;
                }
                match listed {
                    Ok(Listed {
                        files,
                        versions,
                        reader,
                    }) => {
                        source.error = None;
                        let order: Vec<(String, Option<bool>, &ChangedFile)> = match &tab {
                            ReviewTab::File { .. } => unreachable!(),
                            ReviewTab::Changes { path, staged, .. } => {
                                let mut entries: Vec<_> = entries(&files, *staged)
                                    .map(|(id, kind, file)| (id, Some(kind), file))
                                    .collect();
                                // The focused file loads first.
                                if let Some(at) = path.as_ref().and_then(|p| {
                                    entries
                                        .iter()
                                        .position(|e| &e.2.path == p || &e.2.relative == p)
                                }) {
                                    entries[..=at].rotate_right(1);
                                }
                                entries
                            }
                            ReviewTab::Commit { .. } => files
                                .iter()
                                .map(|f| (f.relative.clone(), None, f))
                                .collect(),
                            ReviewTab::Session { path, .. } => {
                                let mut entries: Vec<_> = files
                                    .iter()
                                    .map(|f| (f.relative.clone(), None, f))
                                    .collect();
                                if let Some(at) = path
                                    .as_ref()
                                    .and_then(|p| entries.iter().position(|e| &e.2.path == p))
                                {
                                    entries[..=at].rotate_right(1);
                                }
                                entries
                            }
                        };
                        let retained: HashSet<_> = order.iter().map(|e| e.0.as_str()).collect();
                        source.loaded.retain(|id, _| retained.contains(id.as_str()));
                        let order: Vec<_> = order
                            .into_iter()
                            .map(|(id, staged, file)| (id, staged, file.relative.clone()))
                            .collect();
                        source.files = Some(files.into());
                        this.publish(cx);
                        Some((order, versions, reader))
                    }
                    Err(error) => {
                        source.error = Some(error);
                        source.files = Some(Arc::from([]));
                        source.load = None;
                        source.refresh_pending = false;
                        cx.notify();
                        None
                    }
                }
            });
            let Ok(Some((order, versions, reader))) = order else {
                return;
            };
            let queue = Arc::new(std::sync::Mutex::new(order.into_iter()));
            let versions = versions.map(Arc::new);
            let reader = Arc::new(reader);
            let mut workers = Vec::new();
            for _ in 0..CONCURRENCY {
                let queue = queue.clone();
                let versions = versions.clone();
                let counter = counter.clone();
                let reader = reader.clone();
                let this = this.clone();
                let mut cx = cx.clone();
                workers.push(async move {
                    loop {
                        if counter.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        let Some((id, staged, relative)) = queue
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .next()
                        else {
                            return;
                        };
                        let previous = this
                            .update(&mut cx, |this, _| {
                                this.sources
                                    .iter()
                                    .find(|s| Arc::ptr_eq(&s.generation, &counter))
                                    .and_then(|s| s.loaded.get(&id).cloned())
                            })
                            .ok()
                            .flatten();
                        let version = versions.as_ref().and_then(|v| v.get(&id)).cloned();
                        if previous.as_ref().is_some_and(|p| {
                            p.error.is_none()
                                && (matches!(reader.as_ref(), FileReader::Commit(_))
                                    || (version.is_some() && p.version == version))
                        }) {
                            continue;
                        }
                        let reader = reader.clone();
                        let work_counter = counter.clone();
                        let loaded = cx
                            .background_executor()
                            .spawn(async move {
                                let diff = reader.file(&relative, staged);
                                if work_counter.load(Ordering::SeqCst) != generation {
                                    return None;
                                }
                                match diff {
                                    Ok(diff) => {
                                        if let Some(prev) = previous.filter(|p| {
                                            p.error.is_none()
                                                && p.diff.original == diff.original
                                                && p.diff.current == diff.current
                                                && p.diff.binary == diff.binary
                                                && p.diff.too_large == diff.too_large
                                        }) {
                                            return Some(Arc::new(Loaded {
                                                version,
                                                ..prev.as_ref().clone()
                                            }));
                                        }
                                        let unified =
                                            (!diff.binary && !diff.too_large).then(|| {
                                                diff::build_unified_file(
                                                    &diff.original,
                                                    &diff.current,
                                                    diff::CONTEXT_DEFAULT,
                                                )
                                            });
                                        let (unified, chunks, additions, deletions) = unified
                                            .map_or_else(
                                                || (None, Arc::default(), 0, 0),
                                                |diff| {
                                                    (
                                                        Some(Arc::new(diff.blocks)),
                                                        Arc::new(diff.chunks),
                                                        diff.additions,
                                                        diff.deletions,
                                                    )
                                                },
                                            );
                                        Some(Arc::new(Loaded {
                                            version,
                                            additions,
                                            deletions,
                                            chunks,
                                            unified,
                                            diff: Arc::new(diff),
                                            error: None,
                                        }))
                                    }
                                    Err(error) => Some(Arc::new(Loaded {
                                        version: None,
                                        diff: Arc::new(FileDiff::default()),
                                        unified: None,
                                        chunks: Arc::default(),
                                        additions: 0,
                                        deletions: 0,
                                        error: Some(error),
                                    })),
                                }
                            })
                            .await;
                        let Some(loaded) = loaded else {
                            return;
                        };
                        let _ = this.update(&mut cx, |this, cx| {
                            if counter.load(Ordering::SeqCst) != generation {
                                return;
                            }
                            this.file_loaded(&counter, id, loaded, cx);
                        });
                    }
                });
            }
            futures_join_all(workers).await;
            let _ = this.update(cx, |this, cx| {
                let mut refresh = false;
                let mut publish = false;
                if let Some(source) = this
                    .sources
                    .iter_mut()
                    .find(|s| Arc::ptr_eq(&s.generation, &counter))
                    && counter.load(Ordering::SeqCst) == generation
                {
                    source.complete = source.loaded.values().all(|l| l.error.is_none());
                    source.load = None;
                    publish = source.publish.take().is_some();
                    refresh = std::mem::take(&mut source.refresh_pending);
                }
                if publish {
                    this.publish(cx);
                }
                if refresh {
                    this.load_active(cx);
                }
            });
        });
        if let Some(source) = self.sources.get_mut(ix) {
            source.load = Some(task);
        }
    }

    fn file_loaded(
        &mut self,
        owner: &Arc<AtomicU64>,
        id: String,
        loaded: Arc<Loaded>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .sources
            .get_mut(self.state.active)
            .filter(|source| Arc::ptr_eq(&source.generation, owner))
        else {
            return;
        };
        source.loaded.insert(id, loaded);
        if source.publish.is_none() {
            // The source is current immediately. Merge completions arriving
            // in the same frame before rebuilding the whole review layout.
            let owner = owner.clone();
            let generation = owner.load(Ordering::SeqCst);
            source.publish = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PUBLISH_INTERVAL).await;
                let _ = this.update(cx, |this, cx| {
                    if owner.load(Ordering::SeqCst) == generation
                        && this
                            .sources
                            .get(this.state.active)
                            .is_some_and(|source| Arc::ptr_eq(&source.generation, &owner))
                    {
                        this.flush_publication(cx);
                    }
                });
            }));
        }
    }

    pub(super) fn flush_publication(&mut self, cx: &mut Context<Self>) {
        if self
            .sources
            .get_mut(self.state.active)
            .is_some_and(|source| source.publish.take().is_some())
        {
            self.publish(cx);
        }
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        let ix = self.state.active;
        let Some(source) = self.sources.get(ix) else {
            return;
        };
        let Some(files) = &source.files else {
            cx.notify();
            return;
        };
        let empty = Arc::<Vec<diff::Block>>::default();
        let models: Vec<_> = match &source.tab {
            ReviewTab::File { .. } => return,
            ReviewTab::Changes { staged, .. } => entries(files, *staged)
                .map(|(id, staged, file)| file_model(source, file, id, Some(staged), &empty))
                .collect(),
            ReviewTab::Commit { .. } | ReviewTab::Session { .. } => files
                .iter()
                .map(|file| file_model(source, file, file.relative.clone(), None, &empty))
                .collect(),
        };
        let totals = models
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
        let (focus, file_count) = match &source.tab {
            ReviewTab::File { .. } => return,
            ReviewTab::Changes { path, staged, .. } => (
                path.as_ref().and_then(|p| {
                    entries(files, *staged)
                        .find(|(_, _, f)| &f.path == p || &f.relative == p)
                        .map(|(id, _, _)| id.into())
                }),
                if staged.is_some() {
                    models.len()
                } else {
                    files.len()
                },
            ),
            ReviewTab::Commit { .. } => (None, models.len()),
            ReviewTab::Session { path, .. } => (
                path.as_ref().and_then(|p| {
                    files
                        .iter()
                        .find(|f| &f.path == p)
                        .map(|f| SharedString::from(f.relative.clone()))
                }),
                models.len(),
            ),
        };
        source.view.update(cx, |view, cx| {
            view.totals = Some(totals);
            view.file_count = Some(file_count);
            view.set_files(models, cx);
            view.focus_file(focus, cx);
        });
        cx.notify();
    }
}

fn file_model(
    source: &Source,
    file: &ChangedFile,
    id: String,
    staged: Option<bool>,
    empty: &Arc<Vec<diff::Block>>,
) -> DiffFile {
    let loaded = source.loaded.get(&id);
    let binary = loaded.is_some_and(|loaded| loaded.diff.binary);
    let too_large = loaded.is_some_and(|loaded| loaded.diff.too_large);
    let unified = loaded.and_then(|loaded| loaded.unified.as_ref());
    let unchanged = unified.is_some()
        && loaded.is_some_and(|loaded| loaded.additions == 0 && loaded.deletions == 0)
        && !binary;
    let both_sides = file.staged && file.unstaged;
    let label = match staged.filter(|_| both_sides) {
        Some(true) => format!("{} (Staged)", file.relative),
        Some(false) => format!("{} (Unstaged)", file.relative),
        None => file.relative.clone(),
    };
    let empty_message = match loaded {
        None => Some("Loading…".into()),
        Some(loaded) => loaded
            .error
            .as_ref()
            .map(|error| format!("Couldn’t load diff: {error}"))
            .or_else(|| {
                unchanged.then(|| {
                    match staged {
                        Some(true) => "No staged changes",
                        Some(false) => "No unstaged changes",
                        None => "No textual diff",
                    }
                    .into()
                })
            }),
    };
    let (additions, deletions) = match loaded {
        Some(loaded) if unified.is_some() => (loaded.additions, loaded.deletions),
        _ if staged.is_none() || (!both_sides && loaded.is_some_and(|l| l.error.is_none())) => {
            (file.additions as u32, file.deletions as u32)
        }
        _ => (0, 0),
    };
    let editable = staged == Some(false);
    DiffFile {
        id: id.into(),
        path: file.path.clone(),
        label,
        binary,
        too_large,
        empty_message,
        additions,
        deletions,
        blocks: unified
            .filter(|_| staged.is_none() || !unchanged)
            .unwrap_or(empty)
            .clone(),
        can_stage: editable,
        can_discard: editable,
        can_stage_hunk: editable && !binary && !too_large,
    }
}

/// Polls every future to completion; the workers share one queue.
async fn futures_join_all(workers: Vec<impl std::future::Future<Output = ()>>) {
    let mut workers: Vec<_> = workers.into_iter().map(Box::pin).collect();
    std::future::poll_fn(move |cx| {
        workers.retain_mut(|w| w.as_mut().poll(cx).is_pending());
        if workers.is_empty() {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await
}
