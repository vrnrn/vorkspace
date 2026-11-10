//! The small session projection shared by provider list and detail panes.
use std::{cmp::Reverse, collections::HashMap, rc::Rc};
use vorkspace_core::{inbox::Item, session_groups::WorkItem, store::Session};

type WorkKey = (String, String, u64);

#[derive(Default)]
pub(super) struct RelatedSessions {
    rows: Vec<Rc<SessionLink>>,
    by_url: HashMap<String, Vec<usize>>,
    by_work: HashMap<WorkKey, Vec<usize>>,
}

pub(super) struct SessionLink {
    pub id: i64,
    pub title: String,
    updated_at: u64,
    work: WorkItem,
}

/// The application session directory supplies changed summaries. One index
/// serves all Inbox windows and their retained provider panes.
#[derive(Default)]
pub(crate) struct SessionLinks {
    linked: HashMap<i64, Rc<SessionLink>>,
    index: Rc<RelatedSessions>,
}

impl SessionLinks {
    pub(crate) fn reconcile<'a>(
        &mut self,
        saved: impl IntoIterator<Item = &'a Session>,
        deleted: impl IntoIterator<Item = i64>,
    ) -> bool {
        let mut changed = false;
        for id in deleted {
            changed |= self.linked.remove(&id).is_some();
        }
        for summary in saved {
            let Some(work) = &summary.work_item else {
                changed |= self.linked.remove(&summary.id).is_some();
                continue;
            };
            if self.linked.get(&summary.id).is_some_and(|old| {
                old.title == summary.title
                    && old.updated_at == summary.updated_at
                    && old.work == *work
            }) {
                continue;
            }
            self.linked.insert(
                summary.id,
                Rc::new(SessionLink {
                    id: summary.id,
                    title: summary.title.clone(),
                    updated_at: summary.updated_at,
                    work: work.clone(),
                }),
            );
            changed = true;
        }
        if changed {
            let mut ordered: Vec<_> = self.linked.values().cloned().collect();
            ordered
                .sort_unstable_by_key(|session| (Reverse(session.updated_at), Reverse(session.id)));
            self.index = Rc::new(RelatedSessions::new(ordered));
        }
        changed
    }

    pub(super) fn index(&self) -> Rc<RelatedSessions> {
        self.index.clone()
    }
}

impl RelatedSessions {
    fn new(rows: Vec<Rc<SessionLink>>) -> Self {
        let mut index = Self {
            rows,
            ..Self::default()
        };
        for (row, session) in index.rows.iter().enumerate() {
            let work = &session.work;
            if !work.url.is_empty() {
                index.by_url.entry(work.url.clone()).or_default().push(row);
            }
            index
                .by_work
                .entry((
                    work.kind.clone(),
                    work.repo.to_ascii_lowercase(),
                    work.number,
                ))
                .or_default()
                .push(row);
        }
        index
    }

    pub(super) fn matching(&self, item: &Item) -> impl Iterator<Item = &SessionLink> {
        let key = (
            item.kind.clone(),
            item.repo.to_ascii_lowercase(),
            item.number.max(0) as u64,
        );
        let mut by_url = self.by_url.get(&item.url).into_iter().flatten().peekable();
        let mut by_work = self.by_work.get(&key).into_iter().flatten().peekable();
        // Both lookups are in session display order. A session can match both.
        std::iter::from_fn(move || {
            let row = match (by_url.peek(), by_work.peek()) {
                (Some(url), Some(work)) if url == work => {
                    by_url.next();
                    by_work.next()
                }
                (Some(url), Some(work)) if url < work => by_url.next(),
                (_, Some(_)) => by_work.next(),
                _ => by_url.next(),
            }?;
            Some(self.rows[*row].as_ref())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_sessions_keep_order_and_dedupe_two_matching_routes() {
        let item = Item {
            kind: "pr".into(),
            repo: "owner/repo".into(),
            url: "https://example.test/one".into(),
            number: 7,
            ..Default::default()
        };
        let mut sessions: Vec<_> = [
            (91, "other/repo", true),
            (20, "OWNER/REPO", true),
            (33, "Owner/Repo", false),
            (70, "other/repo", false),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (id, repo, same_url))| Session {
            id,
            title: id.to_string(),
            provider: String::new(),
            cwd: String::new(),
            provider_id: None,
            has_messages: false,
            updated_at: 4 - index as u64,
            archived: false,
            pinned: false,
            work_item: Some(WorkItem {
                kind: "pr".into(),
                repo: repo.into(),
                url: if same_url {
                    item.url.clone()
                } else {
                    String::new()
                },
                number: 7,
            }),
        })
        .collect();
        let mut links = SessionLinks::default();
        assert!(links.reconcile(&sessions, []));
        let index = links.index();
        assert_eq!(
            index.matching(&item).map(|row| row.id).collect::<Vec<_>>(),
            [91, 20, 33]
        );
        sessions[0].pinned = true;
        assert!(!links.reconcile(&sessions[..1], []));
        assert!(Rc::ptr_eq(&index, &links.index()));

        sessions[2].title = "Renamed".into();
        sessions[2].updated_at = 5;
        assert!(links.reconcile(&sessions[2..3], [20]));
        let current = links.index();
        assert_eq!(
            current
                .matching(&item)
                .map(|row| (row.id, row.title.as_str()))
                .collect::<Vec<_>>(),
            [(33, "Renamed"), (91, "91")]
        );
    }
}
