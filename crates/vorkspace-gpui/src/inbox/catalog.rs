//! The small session projection shared by provider list and detail panes.
use std::collections::HashMap;
use vorkspace_core::{inbox::Item, store::Session};

type WorkKey = (String, String, u64);

#[derive(Default)]
pub(super) struct RelatedSessions {
    rows: Vec<SessionLink>,
    by_url: HashMap<String, Vec<usize>>,
    by_work: HashMap<WorkKey, Vec<usize>>,
}

pub(super) struct SessionLink {
    pub id: i64,
    pub title: String,
}

impl RelatedSessions {
    pub(super) fn new(sessions: Vec<Session>) -> Self {
        let mut index = Self::default();
        for session in sessions {
            let Some(work) = session.work_item else {
                continue;
            };
            let row = index.rows.len();
            index.rows.push(SessionLink {
                id: session.id,
                title: session.title,
            });
            if !work.url.is_empty() {
                index.by_url.entry(work.url).or_default().push(row);
            }
            index
                .by_work
                .entry((work.kind, work.repo.to_ascii_lowercase(), work.number))
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
            Some(&self.rows[*row])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vorkspace_core::session_groups::WorkItem;

    #[test]
    fn related_sessions_keep_order_and_dedupe_two_matching_routes() {
        let item = Item {
            kind: "pr".into(),
            repo: "owner/repo".into(),
            url: "https://example.test/one".into(),
            number: 7,
            ..Default::default()
        };
        let index = RelatedSessions::new(
            [
                (91, "other/repo", true),
                (20, "OWNER/REPO", true),
                (33, "Owner/Repo", false),
                (70, "other/repo", false),
            ]
            .into_iter()
            .map(|(id, repo, same_url)| Session {
                id,
                title: id.to_string(),
                provider: String::new(),
                cwd: String::new(),
                provider_id: None,
                has_messages: false,
                updated_at: 0,
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
            .collect(),
        );
        assert_eq!(
            index.matching(&item).map(|row| row.id).collect::<Vec<_>>(),
            [91, 20, 33]
        );
    }
}
