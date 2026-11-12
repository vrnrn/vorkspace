//! Commit sidebar metadata edits and reconcile any rejected optimistic fields.
use super::super::*;
use vorkspace_core::{session_groups::WorkItem, store::Store};

#[derive(Clone)]
pub(super) enum SessionEdit {
    Pin(bool),
    Rename(String),
    Link(Option<WorkItem>),
}

impl SessionEdit {
    fn description(&self) -> &'static str {
        match self {
            Self::Pin(_) => "pin the session",
            Self::Rename(_) => "rename the session",
            Self::Link(_) => "link the session",
        }
    }

    fn read(&self, session: &Session) -> Self {
        match self {
            Self::Pin(_) => Self::Pin(session.pinned),
            Self::Rename(_) => Self::Rename(session.title.clone()),
            Self::Link(_) => Self::Link(session.work_item.clone()),
        }
    }

    fn matches(&self, session: &Session) -> bool {
        match self {
            Self::Pin(pinned) => session.pinned == *pinned,
            Self::Rename(title) => session.title == *title,
            Self::Link(item) => session.work_item == *item,
        }
    }

    fn apply(&self, session: &mut Session) {
        match self {
            Self::Pin(pinned) => session.pinned = *pinned,
            Self::Rename(title) => session.title.clone_from(title),
            Self::Link(item) => session.work_item.clone_from(item),
        }
    }

    fn save(&self, ids: &[i64], store: &Store) -> Result<(), String> {
        match self {
            Self::Pin(pinned) => store.set_pinned(ids, *pinned),
            Self::Rename(title) => ids.iter().try_for_each(|&id| store.title(id, title)),
            Self::Link(item) => ids
                .iter()
                .try_for_each(|&id| store.set_work_item(id, item.as_ref())),
        }
    }
}

struct EditFailure {
    error: String,
    durable: HashMap<i64, SessionEdit>,
}

impl Shell {
    pub(in crate::shell) fn write_session_group(
        &mut self,
        what: &'static str,
        cx: &mut Context<Self>,
        write: impl FnOnce(&mut Store) -> Result<(), String> + Send + 'static,
    ) {
        self.enqueue_group_write(
            Vec::new(),
            cx,
            move |store| write(&mut *locked(&store)?),
            move |shell, result, _, cx| {
                if let Err(error) = result {
                    shell.error = Some(format!("Could not {what}: {error}"));
                    cx.notify();
                }
            },
        );
    }

    fn enqueue_group_write<T: Send + 'static>(
        &mut self,
        changed: Vec<i64>,
        cx: &mut Context<Self>,
        write: impl FnOnce(Database) -> T + Send + 'static,
        complete: impl FnOnce(&mut Self, T, &[i64], &mut Context<Self>) + 'static,
    ) {
        let store = self.store.clone();
        self.begin_session_metadata(&changed, cx);
        let previous = self.sidebar.group_writes.take();
        self.sidebar.group_writes = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx.background_executor().spawn(async move { write(store) });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_session_metadata(&changed, cx);
                complete(this, result, &changed, cx);
            });
        }));
    }

    pub(super) fn edit_session_metadata(
        &mut self,
        ids: Vec<i64>,
        edit: SessionEdit,
        cx: &mut Context<Self>,
    ) {
        let before: Vec<_> = ids
            .iter()
            .map(|&id| (id, edit.read(self.session_metadata(id, cx).summary)))
            .collect();
        for &id in &ids {
            self.apply_session_edit(id, &edit, None, cx);
        }
        let write_ids = ids.clone();
        let requested = edit.clone();
        self.enqueue_group_write(
            ids,
            cx,
            move |store| {
                let db = locked(&store).map_err(|error| EditFailure {
                    error,
                    durable: HashMap::new(),
                })?;
                requested.save(&write_ids, &db).map_err(|error| {
                    // A bulk edit can commit some members before a later member fails.
                    let ids: HashSet<_> = write_ids.into_iter().collect();
                    let durable = db
                        .sessions()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|row| ids.contains(&row.id))
                        .map(|row| (row.id, requested.read(&row)))
                        .collect();
                    EditFailure { error, durable }
                })
            },
            move |shell, result, ids, cx| {
                if let Err(mut failure) = result {
                    for (id, original) in before {
                        let value = failure.durable.remove(&id).unwrap_or(original);
                        shell.apply_session_edit(id, &value, Some(&edit), cx);
                    }
                    shell.error = Some(format!(
                        "Could not {}: {}",
                        edit.description(),
                        failure.error
                    ));
                }
                shell.publish_session_summaries(ids, cx);
                shell.invalidate_chrome(cx);
                cx.notify();
            },
        );
    }

    fn apply_session_edit(
        &mut self,
        id: i64,
        edit: &SessionEdit,
        expected: Option<&SessionEdit>,
        cx: &mut Context<Self>,
    ) {
        let matches = |session: &Session| expected.is_none_or(|edit| edit.matches(session));
        self.edit_session_record(id, cx, |row| {
            if matches(&row.summary) {
                edit.apply(&mut row.summary);
            }
        });
        if let Some(view) = self.sessions.get(&id) {
            view.update(cx, |view, cx| {
                if matches(&view.summary) && !edit.matches(&view.summary) {
                    match edit {
                        SessionEdit::Rename(title) => view.rename(title.clone(), cx),
                        _ => {
                            edit.apply(&mut view.summary);
                            cx.notify();
                        }
                    }
                }
            });
        }
        if self.has_session(id, cx) {
            self.sync_search_session(id, cx);
        }
    }
}
