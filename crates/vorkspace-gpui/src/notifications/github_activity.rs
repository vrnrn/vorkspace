//! Shared linked-work activity and its read-only GitHub projection.
use gpui::{Context, EventEmitter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Instant;
use std::{collections::HashMap, path::Path, time::Duration};
use vorkspace_core::{
    session_groups::WorkItem,
    store::{Session, Store},
};
pub(crate) const SEEN_KEY: &str = "linked-work-item-seen";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub id: String,
    pub kind: String,
    pub author: String,
    pub text: String,
    pub created_at: String,
    pub url: String,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Card {
    pub session: i64,
    pub item: WorkItem,
    pub title: String,
    pub state: String,
    pub updated_at: i64,
    pub entries: Vec<Entry>,
    pub details_error: bool,
    pub truncated: bool,
}
impl Card {
    pub(crate) fn summary(&self) -> String {
        let count = |kind: &str| {
            self.entries
                .iter()
                .filter(|e| {
                    if kind == "comment" {
                        e.kind == "comment" || e.kind == "review_comment"
                    } else {
                        e.kind == kind
                    }
                })
                .count()
        };
        let parts = [
            (count("commit"), "commit"),
            (count("review"), "review"),
            (count("comment"), "comment"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, k)| format!("{n} new {k}{}", if n == 1 { "" } else { "s" }))
        .collect::<Vec<_>>();
        if !parts.is_empty() {
            parts.join(" · ")
        } else if self.details_error {
            "Updated on GitHub · details unavailable".into()
        } else {
            "Metadata or status changed".into()
        }
    }
    pub(crate) fn terminal_label(&self) -> Option<&'static str> {
        match (
            self.item.kind.as_str(),
            self.state.to_ascii_lowercase().as_str(),
        ) {
            ("pr", "merged") => Some("Pull request merged"),
            ("pr", "closed") => Some("Pull request closed"),
            ("issue", "closed") => Some("Issue closed"),
            _ => None,
        }
    }
    pub(crate) fn prompt(&self) -> String {
        let first = self.entries.first();
        let instruction = match first.map(|e| e.kind.as_str()) {
            Some("commit") => "Review the new commit and continue the work where needed.",
            Some(_) => "Address the new feedback and continue the work where needed.",
            None => "Review the latest update and continue the work where needed.",
        };
        let details = if self.entries.is_empty() {
            vec![format!(
                "- {}; current state: {}",
                self.summary(),
                self.state
            )]
        } else {
            self.entries
                .iter()
                .take(12)
                .map(|e| {
                    format!(
                        "- {}{}: {}",
                        e.kind.replace('_', " "),
                        if e.author.is_empty() {
                            String::new()
                        } else {
                            format!(" by @{}", e.author)
                        },
                        if e.text.is_empty() {
                            "No message"
                        } else {
                            &e.text
                        }
                    )
                })
                .collect()
        };
        format!(
            "{instruction}\n\nThe linked GitHub {} has new activity:\n\n#{} {}\n{}\n\n{}",
            if self.item.kind == "pr" {
                "pull request"
            } else {
                "issue"
            },
            self.item.number,
            self.title,
            self.item.url,
            details.join("\n")
        )
    }
}
pub(crate) fn load_seen(store: &Store) -> Result<HashMap<i64, i64>, String> {
    Ok(store
        .setting(SEEN_KEY)?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}
/// Merge the desktop's acknowledged UUID snapshots with the native cache.
/// Resolve source IDs through Store rather than treating UUIDs as native IDs.
#[derive(Clone)]
pub(crate) struct LinkedSession {
    pub(crate) id: i64,
    pub(crate) updated_at: i64,
    pub(crate) item: WorkItem,
}

impl LinkedSession {
    pub(crate) fn from_summary(summary: &Session) -> Option<Self> {
        if summary.archived {
            return None;
        }
        Some(Self {
            id: summary.id,
            updated_at: summary.updated_at.min(i64::MAX as u64) as i64,
            item: summary.work_item.clone()?,
        })
    }
}

pub(crate) enum GithubActivityEvent {
    Changed { new_activity: Vec<i64> },
    Acknowledged(String),
}

#[derive(Default)]
pub(crate) struct GithubActivity {
    pub cards: HashMap<i64, Card>,
    seen: Option<HashMap<i64, i64>>,
    revisions: HashMap<i64, (WorkItem, i64)>,
    primed: bool,
    checked: Option<Instant>,
    checking: bool,
}

impl EventEmitter<GithubActivityEvent> for GithubActivity {}

impl GithubActivity {
    pub(crate) fn restore_seen(&mut self, seen: HashMap<i64, i64>) {
        self.seen.get_or_insert(seen);
    }

    pub(crate) fn acknowledge(&mut self, session: i64, cx: &mut Context<Self>) {
        let Some(card) = self.cards.remove(&session) else {
            return;
        };
        let seen = self.seen.get_or_insert_with(HashMap::new);
        let current = seen.entry(session).or_default();
        *current = (*current).max(card.updated_at);
        if seen.len() > 500 {
            let mut entries: Vec<_> = seen.iter().map(|(id, at)| (*id, *at)).collect();
            entries.sort_unstable_by_key(|(id, at)| std::cmp::Reverse((*at, *id)));
            *seen = entries.into_iter().take(500).collect();
        }
        let encoded = serde_json::to_string(seen).expect("serialize linked activity timestamps");
        cx.emit(GithubActivityEvent::Acknowledged(encoded));
        cx.emit(GithubActivityEvent::Changed {
            new_activity: Vec::new(),
        });
    }

    pub(crate) fn begin_poll(&mut self) -> Option<HashMap<i64, i64>> {
        let seen = self.seen.as_ref()?;
        if self.checking
            || self
                .checked
                .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
        {
            return None;
        }
        self.checked = Some(Instant::now());
        self.checking = true;
        Some(seen.clone())
    }

    pub(crate) fn finish_poll(
        &mut self,
        mut cards: HashMap<i64, Card>,
        current: &HashMap<i64, LinkedSession>,
        cx: &mut Context<Self>,
    ) {
        self.checking = false;
        let Some(seen) = &self.seen else {
            return;
        };
        // Ignore results for a link that was removed/replaced, an archived
        // conversation, a new local turn, or an acknowledgment during the poll.
        cards.retain(|id, card| {
            current.get(id).is_some_and(|session| {
                let since = session.updated_at.max(*seen.get(id).unwrap_or(&0));
                if card.item != session.item || card.updated_at <= since {
                    return false;
                }
                card.entries
                    .retain(|entry| timestamp(&entry.created_at) > since);
                true
            })
        });
        self.revisions
            .retain(|id, (item, _)| current.get(id).is_some_and(|session| session.item == *item));
        let mut new_activity = Vec::new();
        for (&id, card) in &cards {
            let previous = self
                .revisions
                .entry(id)
                .or_insert_with(|| (card.item.clone(), 0));
            if self.primed && card.updated_at > previous.1 && !card.details_error {
                new_activity.push(id);
            }
            previous.1 = previous.1.max(card.updated_at);
        }
        self.primed = true;
        if self.cards != cards {
            self.cards = cards;
            cx.emit(GithubActivityEvent::Changed { new_activity });
        }
    }
}

fn timestamp(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}
fn api(endpoint: &str) -> Result<Value, String> {
    let binary = vorkspace_core::scm::resolve_gui_binary("gh").ok_or("GitHub CLI not found")?;
    let text = vorkspace_core::cli::output(
        &binary,
        &[
            "api".into(),
            "--method".into(),
            "GET".into(),
            endpoint.into(),
            "--jq".into(),
            r#"if type == "array" then map({id,sha,user:{login:.user.login},author:{login:.author.login},body:((.body // "")[0:256]),state,submitted_at,created_at,html_url,commit:{message:((.commit.message // "")[0:256]),committer:{date:.commit.committer.date}}}) else {title,state,merged,updated_at} end"#.into(),
        ],
        Path::new("/"),
        Duration::from_secs(20),
    )?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}
pub(crate) fn fetch(
    sessions: HashMap<i64, LinkedSession>,
    seen: &HashMap<i64, i64>,
) -> HashMap<i64, Card> {
    let mut groups: HashMap<String, Vec<LinkedSession>> = HashMap::new();
    for session in sessions.into_values() {
        let item = &session.item;
        let collection = match item.kind.as_str() {
            "pr" => "pulls",
            "issue" => "issues",
            _ => continue,
        };
        let slug = item.repo.split_once('/');
        let valid = |part: &str| {
            !matches!(part, "" | "." | "..")
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        };
        if item.number == 0 || !slug.is_some_and(|(owner, repo)| valid(owner) && valid(repo)) {
            continue;
        }
        let base = format!(
            "repos/{}/{collection}/{}",
            item.repo.to_ascii_lowercase(),
            item.number
        );
        groups.entry(base).or_default().push(session);
    }

    let mut cards = HashMap::new();
    for (base, sessions) in groups {
        let Ok(body) = api(&base) else {
            continue;
        };
        let updated_at = timestamp(body["updated_at"].as_str().unwrap_or(""));
        let mut affected = sessions
            .into_iter()
            .filter_map(|session| {
                let since = session.updated_at.max(*seen.get(&session.id).unwrap_or(&0));
                (updated_at > since).then_some((session, since))
            })
            .peekable();
        let Some((first, _)) = affected.peek() else {
            continue;
        };
        let issue_base = format!(
            "repos/{}/issues/{}",
            first.item.repo.to_ascii_lowercase(),
            first.item.number
        );
        let mut feeds = vec![(format!("{issue_base}/comments?per_page=100"), "comment")];
        if first.item.kind == "pr" {
            feeds.extend([
                (format!("{base}/reviews?per_page=100"), "review"),
                (format!("{base}/comments?per_page=100"), "review_comment"),
                (format!("{base}/commits?per_page=100"), "commit"),
            ]);
        }
        let mut entries = Vec::new();
        let mut details_error = false;
        let mut truncated = false;
        for (url, kind) in feeds {
            let Ok(body) = api(&url) else {
                details_error = true;
                continue;
            };
            let Some(feed) = body.as_array() else {
                details_error = true;
                continue;
            };
            truncated |= feed.len() == 100;
            entries.extend(feed.iter().map(|entry| read_entry(entry, kind)));
        }
        entries.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        let title = body["title"].as_str().unwrap_or("");
        let state = if body["merged"].as_bool() == Some(true) {
            "merged"
        } else {
            body["state"].as_str().unwrap_or("")
        };
        for (session, since) in affected {
            cards.insert(
                session.id,
                Card {
                    session: session.id,
                    item: session.item,
                    title: title.into(),
                    state: state.into(),
                    updated_at,
                    entries: entries
                        .iter()
                        .take_while(|(at, _)| *at > since)
                        .map(|(_, entry)| entry.clone())
                        .collect(),
                    details_error,
                    truncated,
                },
            );
        }
    }
    cards
}

fn read_entry(entry: &Value, kind: &str) -> (i64, Entry) {
    let commit = kind == "commit";
    let at = if commit {
        entry["commit"]["committer"]["date"].as_str()
    } else if kind == "review" {
        entry["submitted_at"].as_str()
    } else {
        entry["created_at"].as_str()
    }
    .unwrap_or("");
    let text = if commit {
        entry["commit"]["message"]
            .as_str()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .into()
    } else {
        let body = entry["body"].as_str().unwrap_or("");
        if kind == "review" {
            format!(
                "{}: {body}",
                entry["state"].as_str().unwrap_or("").replace('_', " ")
            )
        } else {
            body.to_owned()
        }
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = if text.chars().count() > 140 {
        format!("{}…", text.chars().take(139).collect::<String>())
    } else {
        text
    };
    (
        timestamp(at),
        Entry {
            id: if commit {
                entry["sha"].as_str().unwrap_or("").into()
            } else {
                entry["id"].to_string()
            },
            kind: kind.into(),
            author: entry["user"]["login"]
                .as_str()
                .or_else(|| entry["author"]["login"].as_str())
                .unwrap_or("")
                .into(),
            text,
            created_at: at.into(),
            url: entry["html_url"].as_str().unwrap_or("").into(),
        },
    )
}
