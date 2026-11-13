//! One app-wide directory for saved session metadata. Live chat entities
//! still belong to their window, as do tabs, ordering and navigation.
use super::*;
use vorkspace_core::store::SessionSidebarMetadata;

#[derive(Clone, PartialEq)]
pub(super) struct SessionRecord {
    pub summary: Session,
    /// A missing lookup differs from a known session in its main checkout.
    pub checkout: Option<Option<String>>,
    pub draft: bool,
}

impl SessionRecord {
    pub fn worktree(&self) -> Option<&str> {
        self.checkout.as_ref().and_then(Option::as_deref)
    }
}

#[derive(Default)]
pub(super) struct SessionDirectory {
    pub rows: HashMap<i64, SessionRecord>,
}

impl SessionDirectory {
    pub fn reconcile(
        &mut self,
        sessions: &[Session],
        metadata: &SessionSidebarMetadata,
        protected: &HashSet<i64>,
        internal: &HashSet<i64>,
    ) -> Vec<i64> {
        let mut changed = Vec::new();
        for summary in sessions {
            if protected.contains(&summary.id) || internal.contains(&summary.id) {
                continue;
            }
            let checkout = metadata
                .worktrees
                .get(&summary.id)
                .or_else(|| self.rows.get(&summary.id)?.checkout.as_ref());
            let draft = metadata.drafts.contains(&summary.id);
            if self.rows.get(&summary.id).is_some_and(|row| {
                &row.summary == summary && row.checkout.as_ref() == checkout && row.draft == draft
            }) {
                continue;
            }
            self.rows.insert(
                summary.id,
                SessionRecord {
                    summary: summary.clone(),
                    checkout: checkout.cloned(),
                    draft,
                },
            );
            changed.push(summary.id);
        }
        changed
    }
}

impl Shell {
    pub(super) fn session_record<'a>(&'a self, id: i64, cx: &'a App) -> Option<&'a SessionRecord> {
        self.directory.read(cx).rows.get(&id)
    }

    pub(super) fn session_summary<'a>(&'a self, id: i64, cx: &'a App) -> Option<&'a Session> {
        self.session_record(id, cx)
            .map(|record| &record.summary)
            .or_else(|| self.sessions.get(&id).map(|view| &view.read(cx).summary))
    }

    pub(super) fn has_session(&self, id: i64, cx: &App) -> bool {
        self.session_summary(id, cx).is_some()
    }

    pub(super) fn put_session_record(&self, record: SessionRecord, cx: &mut Context<Self>) {
        self.directory.update(cx, |directory, _| {
            directory.rows.insert(record.summary.id, record);
        });
    }

    pub(super) fn edit_session_record(
        &self,
        id: i64,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut SessionRecord),
    ) {
        self.directory.update(cx, |directory, _| {
            if let Some(record) = directory.rows.get_mut(&id) {
                edit(record);
            }
        });
    }

    pub(super) fn set_session_worktree(
        &self,
        id: i64,
        path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self
            .session_record(id, cx)
            .is_some_and(|record| record.checkout.as_ref() != Some(&path))
        {
            self.edit_session_record(id, cx, |row| row.checkout = Some(path));
            self.publish_session_summary(id, cx);
        }
    }

    #[cfg(test)]
    pub(super) fn seed_session_summary(&self, summary: Session, cx: &mut Context<Self>) {
        self.put_session_record(
            SessionRecord {
                summary,
                checkout: None,
                draft: false,
            },
            cx,
        );
    }
}
