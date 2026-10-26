//! Read-only GitHub polling for linked session notifications. No GitHub calls
//! occur until the shell invokes poll; acknowledgment persistence stays in shell.
use gpui::{Context, EventEmitter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Instant;
use std::{collections::HashMap, path::Path, time::Duration};
use vorkspace_core::{
    session_groups::WorkItem,
    store::{Session, Store},
};
const SEEN_KEY: &str = "linked-work-item-seen";
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
pub(crate) fn load_seen_with_metadata(
    store: &Store,
    metadata: Option<&Value>,
) -> Result<HashMap<i64, i64>, String> {
    let mut seen = load_seen(store)?;
    let raw = metadata.and_then(|m| m.get("vorkspace.linkedSessionSeen"));
    let decoded = raw
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let values = decoded.as_ref().or(raw).and_then(Value::as_object);
    if let Some(values) = values {
        for (source_id, timestamp) in values {
            if let (Some(id), Some(at)) = (
                store.resolve_existing_session_id(source_id),
                timestamp.as_i64(),
            ) {
                let current = seen.entry(id).or_default();
                *current = (*current).max(at);
            }
        }
    }
    Ok(seen)
}
pub(crate) fn save_seen(store: &Store, seen: &HashMap<i64, i64>) -> Result<(), String> {
    store.save_setting(
        SEEN_KEY,
        &serde_json::to_string(seen).map_err(|e| e.to_string())?,
    )
}
pub(crate) enum GithubActivityEvent {
    Changed,
    Acknowledged(HashMap<i64, i64>),
}
pub(crate) struct GithubActivity {
    pub cards: HashMap<i64, Card>,
    seen: HashMap<i64, i64>,
    checked: Option<Instant>,
    checking: bool,
}
impl EventEmitter<GithubActivityEvent> for GithubActivity {}
impl GithubActivity {
    pub(crate) fn new(seen: HashMap<i64, i64>) -> Self {
        Self {
            cards: HashMap::new(),
            seen,
            checked: None,
            checking: false,
        }
    }
    pub(crate) fn restore_seen(&mut self, seen: HashMap<i64, i64>) {
        for (id, at) in seen {
            let current = self.seen.entry(id).or_default();
            *current = (*current).max(at);
        }
        self.cards
            .retain(|id, card| card.updated_at > *self.seen.get(id).unwrap_or(&0));
    }
    pub(crate) fn acknowledge(&mut self, session: i64, cx: &mut Context<Self>) {
        if let Some(card) = self.cards.remove(&session) {
            let seen = self.seen.entry(session).or_default();
            *seen = (*seen).max(card.updated_at);
            if self.seen.len() > 500 {
                let mut entries: Vec<_> = self.seen.iter().map(|(id, at)| (*id, *at)).collect();
                entries.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
                self.seen = entries.into_iter().take(500).collect();
            }
            cx.emit(GithubActivityEvent::Acknowledged(self.seen.clone()));
            cx.emit(GithubActivityEvent::Changed);
            cx.notify();
        }
    }
    pub(crate) fn ready_to_poll(&self) -> bool {
        !self.checking
            && self
                .checked
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(60))
    }
    pub(crate) fn poll(&mut self, sessions: Vec<Session>, cx: &mut Context<Self>) {
        if self.checking
            || self
                .checked
                .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        self.checking = true;
        self.checked = Some(Instant::now());
        let seen = self.seen.clone();
        let work = cx
            .background_executor()
            .spawn(async move { fetch(sessions, &seen) });
        cx.spawn(async move |this, cx| {
            let cards = work.await;
            let _ = this.update(cx, |this, cx| {
                this.checking = false;
                // An acknowledgment made while polling must not reappear.
                this.cards = cards
                    .into_iter()
                    .filter(|(id, c)| c.updated_at > *this.seen.get(id).unwrap_or(&0))
                    .collect();
                cx.emit(GithubActivityEvent::Changed);
                cx.notify();
            });
        })
        .detach();
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
fn fetch(sessions: Vec<Session>, seen: &HashMap<i64, i64>) -> HashMap<i64, Card> {
    let mut cards = HashMap::new();
    let mut cache: HashMap<String, Option<Value>> = HashMap::new();
    for session in sessions
        .into_iter()
        .filter(|s| !s.archived && s.work_item.is_some())
    {
        let item = session.work_item.unwrap();
        // Only canonical GitHub repository slugs enter a gh API endpoint.
        let parts = item.repo.split('/').collect::<Vec<_>>();
        if item.number == 0
            || parts.len() != 2
            || parts.iter().any(|p| {
                p.is_empty()
                    || !p
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            })
        {
            continue;
        }
        let base = format!(
            "repos/{}/{}s/{}",
            item.repo,
            if item.kind == "pr" { "pull" } else { "issue" },
            item.number
        );
        let Some(body) = cache
            .entry(base.clone())
            .or_insert_with(|| api(&base).ok())
            .clone()
        else {
            continue;
        };
        let updated_at = timestamp(body["updated_at"].as_str().unwrap_or(""));
        let since = (session.updated_at.min(i64::MAX as u64) as i64)
            .max(*seen.get(&session.id).unwrap_or(&0));
        if updated_at <= since {
            continue;
        }
        let mut card = Card {
            session: session.id,
            title: body["title"].as_str().unwrap_or("").into(),
            state: if body["merged"].as_bool() == Some(true) {
                "merged".into()
            } else {
                body["state"].as_str().unwrap_or("").into()
            },
            updated_at,
            item,
            entries: Vec::new(),
            details_error: false,
            truncated: false,
        };
        let issue_base = format!("repos/{}/issues/{}", card.item.repo, card.item.number);
        let mut feeds = vec![(format!("{issue_base}/comments?per_page=100"), "comment")];
        if card.item.kind == "pr" {
            feeds.extend([
                (format!("{base}/reviews?per_page=100"), "review"),
                (format!("{base}/comments?per_page=100"), "review_comment"),
                (format!("{base}/commits?per_page=100"), "commit"),
            ]);
        }
        for (url, kind) in feeds {
            let result = api(&url);
            let Ok(body) = result else {
                card.details_error = true;
                continue;
            };
            let Some(entries) = body.as_array() else {
                card.details_error = true;
                continue;
            };
            card.truncated |= entries.len() == 100;
            for entry in entries {
                let commit = kind == "commit";
                let at = if commit {
                    entry["commit"]["committer"]["date"].as_str()
                } else if kind == "review" {
                    entry["submitted_at"].as_str()
                } else {
                    entry["created_at"].as_str()
                }
                .unwrap_or("");
                if timestamp(at) <= since {
                    continue;
                }
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
                card.entries.push(Entry {
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
                });
            }
        }
        card.entries
            .sort_by_key(|e| std::cmp::Reverse(timestamp(&e.created_at)));
        cards.insert(card.session, card);
    }
    cards
}
