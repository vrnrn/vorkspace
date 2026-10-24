//! Native Inbox services and the desktop app's existing credential formats.
mod avatar;
mod bounded;
mod connectors;
pub use connectors::Connectors;
mod preferences;
pub use preferences::{BotDefaults, JiraFilter};
mod notifications;
pub use notifications::NotificationProject;
mod media;
pub use avatar::png as avatar_png;
pub use media::png as media_png;
mod github;
pub mod gitlab;
pub mod jira;

use crate::{session_groups::WorkItem, store::Store};
use chrono::{Datelike, Local, TimeZone};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
};

pub const PROVIDERS: [&str; 3] = ["github", "gitlab", "jira"];
pub fn provider_label(provider: &str) -> &'static str {
    match provider {
        "jira" => "Jira",
        "gitlab" => "GitLab",
        "github" => "GitHub",
        _ => "Unknown",
    }
}
pub fn is_tracker(provider: &str) -> bool {
    provider == "jira"
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Label {
    pub name: String,
    pub color: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Person {
    pub login: String,
    pub avatar_url: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Item {
    pub provider: String,
    pub kind: String,
    pub id: String,
    pub identifier: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub state_type: String,
    pub state_reason: String,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<Label>,
    pub assignees: Vec<Person>,
    pub draft: bool,
    pub repo: String,
    pub team_id: String,
    pub team_name: String,
    pub project_path: String,
    pub attention_reason: String,
}
/// A successful write's URL and server-authored revision, when available.
/// Missing revision metadata never turns a completed write into a failure.
#[derive(Debug)]
pub struct CommentReceipt {
    pub url: String,
    pub revision: Option<String>,
}
impl Item {
    pub fn key(&self) -> String {
        let identity = if self.provider == "jira" {
            let id = if self.id.trim().is_empty() {
                self.identifier.trim()
            } else {
                self.id.trim()
            };
            if id.is_empty() {
                format!("jira:{}", self.number)
            } else {
                id.to_lowercase()
            }
        } else if !self.repo.trim().is_empty() {
            format!(
                "{}:{}:{}",
                self.repo.trim().to_lowercase(),
                self.kind,
                self.number
            )
        } else if !self.url.trim().is_empty() {
            self.url.trim().to_lowercase()
        } else {
            format!("{}:{}", self.kind, self.number)
        };
        format!("{}:{identity}", self.provider)
    }
    pub fn reference(&self) -> String {
        if self.identifier.is_empty() {
            format!("#{}", self.number)
        } else {
            self.identifier.clone()
        }
    }
    pub fn updated_ms(&self) -> i64 {
        timestamp_ms(&self.updated_at)
    }
    pub fn status(&self) -> &'static str {
        if self.state.eq_ignore_ascii_case("merged") {
            "Merged"
        } else if matches!(
            self.state.to_lowercase().as_str(),
            "closed" | "done" | "resolved" | "removed" | "completed"
        ) || matches!(self.state_type.as_str(), "completed" | "canceled" | "done")
        {
            "Closed"
        } else if self.draft {
            "Draft"
        } else {
            "Open"
        }
    }
    pub fn work_item(&self) -> WorkItem {
        WorkItem {
            kind: self.kind.clone(),
            repo: self.repo.clone(),
            number: self.number.max(0) as u64,
            url: self.url.clone(),
        }
    }
    pub fn prompt(&self, body: &str, ask: bool) -> String {
        format!(
            "{}\n\n{} {}: {}\n{}\n\n{}",
            if ask {
                "Help me understand and discuss this work item."
            } else {
                "Work on this item. Read its description and discussion, inspect the project, and implement the required changes."
            },
            provider_label(&self.provider),
            self.reference(),
            self.title,
            self.url,
            body
        )
    }
}
pub fn timestamp_ms(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|at| at.timestamp_millis())
        .unwrap_or(0)
}
pub fn relative_time(value: &str) -> String {
    let at = timestamp_ms(value);
    if at == 0 {
        return String::new();
    }
    let seconds = ((crate::time::now_ms() as i64 - at).max(0)) / 1000;
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        86400..604800 => format!("{}d", seconds / 86400),
        _ => chrono::DateTime::from_timestamp_millis(at)
            .map(|d| d.format("%b %-d").to_string())
            .unwrap_or_default(),
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Details {
    pub body: String,
    pub author: String,
    pub author_avatar_url: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub review_decision: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub kind: String,
    pub author: String,
    pub author_avatar_url: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
    pub state: String,
    pub path: String,
    pub line: Option<i64>,
    pub resolved: bool,
    pub thread_id: String,
    pub replies: Vec<Comment>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Commit {
    pub oid: String,
    pub message_headline: String,
    pub author: String,
    pub committed_date: String,
    pub url: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Thread {
    pub comments: Vec<Comment>,
    pub commits: Vec<Commit>,
    pub truncated: bool,
    pub review_decision: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiffFile {
    pub path: String,
    pub additions: i64,
    pub deletions: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Diff {
    pub additions: i64,
    pub deletions: i64,
    pub files: Vec<DiffFile>,
    pub patch: String,
    pub truncated: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub workflow: String,
    pub state: String,
    pub url: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Checks {
    pub head_oid: String,
    pub checks: Vec<Check>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Status {
    pub connected: bool,
    pub installed: bool,
    pub authenticated: bool,
    pub url: String,
    pub site: String,
    pub email: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OptionRow {
    pub id: String,
    pub name: String,
    pub key: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StatusFilter {
    pub open: bool,
    pub draft: bool,
    pub closed: bool,
    pub merged: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Filters {
    pub assigned_to_me: bool,
    pub hidden_projects: Vec<String>,
    pub hidden_kinds: Vec<String>,
    pub time: String,
    pub status: StatusFilter,
}
impl Default for Filters {
    fn default() -> Self {
        Self {
            assigned_to_me: false,
            hidden_projects: vec![],
            hidden_kinds: vec![],
            time: "all".into(),
            status: StatusFilter::default(),
        }
    }
}
impl Filters {
    pub fn fetch_state(&self) -> &str {
        if self.status.closed || self.status.merged || !(self.status.open || self.status.draft) {
            "all"
        } else {
            "open"
        }
    }
    pub fn matches(&self, item: &Item, source: &str, search: &str) -> bool {
        if item.provider != source {
            return false;
        }
        let tracker = is_tracker(source);
        if !tracker
            && !(source == "gitlab" && self.assigned_to_me)
            && self.hidden_projects.contains(&item.project_path)
        {
            return false;
        }
        if !tracker && self.hidden_kinds.contains(&item.kind) {
            return false;
        }
        let s = &self.status;
        if (s.open || s.closed || (!tracker && (s.draft || s.merged)))
            && !match item.status() {
                "Open" => s.open,
                "Draft" => s.draft && !tracker,
                "Closed" => s.closed,
                "Merged" => s.merged && !tracker,
                _ => true,
            }
        {
            return false;
        }
        let now = Local::now();
        let start = match self.time.as_str() {
            "today" => Local
                .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
                .single()
                .map(|d| d.timestamp_millis())
                .unwrap_or(0),
            "7d" => now.timestamp_millis() - 7 * 86400000,
            "30d" => now.timestamp_millis() - 30 * 86400000,
            _ => 0,
        };
        if start > 0 && item.updated_ms() < start {
            return false;
        }
        let text = format!(
            "{} {} {} {} {} {}",
            item.title,
            item.reference(),
            item.number,
            item.repo,
            item.team_name,
            item.labels
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        )
        .to_lowercase();
        search
            .split_whitespace()
            .all(|word| text.contains(&word.to_lowercase()))
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Seen {
    pub seeded: bool,
    pub items: BTreeMap<String, i64>,
}
impl Seen {
    pub fn unseen(&self, item: &Item) -> bool {
        self.seeded
            && self
                .items
                .get(&item.key())
                .is_none_or(|at| item.updated_ms() > *at)
    }
    pub fn mark(&mut self, item: &Item) {
        let old = self.items.entry(item.key()).or_default();
        *old = (*old).max(item.updated_ms());
    }
    pub fn seed(&mut self, items: &[Item]) {
        if !self.seeded && !items.is_empty() {
            for item in items {
                self.mark(item);
            }
            self.seeded = true;
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewState {
    pub source: String,
    #[serde(skip)]
    pub connectors: Connectors,
    #[serde(skip)]
    pub jira_filter: JiraFilter,
    #[serde(skip)]
    pub default_bots: BotDefaults,
    pub filters: Filters,
    pub seen: Seen,
    pub hidden_jira_projects: Vec<String>,
    pub list_width: f32,
}
impl Default for ViewState {
    fn default() -> Self {
        Self {
            source: "github".into(),
            connectors: Connectors::default(),
            jira_filter: JiraFilter {
                mode: "project".into(),
                jql: String::new(),
            },
            default_bots: BotDefaults::default(),
            filters: Filters::default(),
            seen: Seen::default(),
            hidden_jira_projects: vec![],
            list_width: 280.,
        }
    }
}
impl ViewState {
    pub fn fetch_scope(&self, projects: &[PathBuf]) -> String {
        let mut paths = projects
            .iter()
            .map(|p| crate::files::path_key(&p.to_string_lossy()))
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        let mut jira = self.hidden_jira_projects.clone();
        jira.sort();
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.filters.assigned_to_me,
            self.filters.fetch_state(),
            paths.join("|"),
            self.connectors
                .enabled_providers()
                .collect::<Vec<_>>()
                .join(","),
            jira.join(","),
            serde_json::to_string(&self.jira_filter).unwrap_or_default()
        )
    }
    pub fn load(store: &Store) -> Self {
        if let Some(mut state) = store
            .setting("native-inbox-state")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
        {
            state.connectors = Connectors::load(store);
            state.jira_filter = JiraFilter::load(store);
            state.default_bots = BotDefaults::load(store);
            state.normalize_source();
            return state;
        }
        let desktop = store.desktop_preferences().cloned().unwrap_or_default();
        let mut state = Self::default();
        if let Some(source) = desktop
            .get("vorkspace.inboxSource")
            .filter(|s| PROVIDERS.contains(s))
        {
            state.source = source.into();
        }
        if let Some(filters) = desktop
            .json("vorkspace.inboxFilters")
            .and_then(|v| serde_json::from_value(v).ok())
        {
            state.filters = filters;
        }
        if let Some(seen) = desktop.json("vorkspace.inboxSeen") {
            state.seen = serde_json::from_value(seen.clone()).unwrap_or_else(|_| Seen {
                seeded: true,
                items: serde_json::from_value(seen).unwrap_or_default(),
            });
        }
        state.hidden_jira_projects = desktop
            .json("vorkspace.jiraHiddenProjects")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        state.connectors = Connectors::load(store);
        state.jira_filter = JiraFilter::load(store);
        state.default_bots = BotDefaults::load(store);
        state.normalize_source();
        state
    }
    pub fn normalize_source(&mut self) {
        if !self.connectors.enabled(&self.source) {
            self.source = self
                .connectors
                .enabled_providers()
                .next()
                .unwrap_or("")
                .into();
        }
    }
    pub fn save(&self, store: &Store) -> Result<(), String> {
        store.save_setting(
            "native-inbox-state",
            &serde_json::to_string(self).map_err(|e| e.to_string())?,
        )
    }
}
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub items: Vec<Item>,
    pub errors: BTreeMap<String, String>,
    pub connections: BTreeMap<String, Status>,
    pub jira_projects: Vec<OptionRow>,
}

/// Provider notification baselines and revisions are
/// separate from the user's read state. Startup and query changes never
/// trigger automations for an existing backlog.
#[derive(Default)]
pub struct NotificationTracker {
    revisions: HashMap<String, i64>,
    primed: HashSet<String>,
    scope: Option<String>,
}
#[derive(Default)]
pub struct Observation {
    pub appeared: Vec<Item>,
    pub changed: Vec<Item>,
}
impl NotificationTracker {
    pub fn observe(&mut self, snapshot: &Snapshot, scope: String) -> Vec<Item> {
        self.observe_snapshot(snapshot, scope).appeared
    }
    pub fn observe_snapshot(&mut self, snapshot: &Snapshot, scope: String) -> Observation {
        if self.scope.as_ref() != Some(&scope) {
            self.primed.clear();
        }
        self.scope = Some(scope);
        let mut observation = Observation::default();
        for item in &snapshot.items {
            if !PROVIDERS.contains(&item.provider.as_str())
                || snapshot.errors.contains_key(&item.provider)
            {
                continue;
            }
            let at = item.updated_ms();
            if at == 0 {
                continue;
            }
            let project = item.notification_project().id;
            let id = if item.id.is_empty() {
                item.number.to_string()
            } else {
                item.id.clone()
            };
            let key = format!("{project}:{}:{id}", item.kind);
            let previous = self.revisions.get(&key).copied();
            if self.primed.contains(&item.provider) && previous.is_none() {
                observation.appeared.push(item.clone());
                observation.changed.push(item.clone());
            } else if self.primed.contains(&item.provider)
                && previous.is_some_and(|previous| at > previous)
            {
                observation.changed.push(item.clone());
            }
            self.revisions.insert(key, previous.unwrap_or(0).max(at));
        }
        for provider in PROVIDERS {
            if !snapshot.errors.contains_key(provider) {
                self.primed.insert(provider.into());
            }
        }
        observation
    }
}

#[derive(Clone)]
pub struct Service {
    pub data_dir: PathBuf,
    pub cwd: PathBuf,
}
fn convert<T: Serialize, U: DeserializeOwned>(value: T) -> Result<U, String> {
    serde_json::from_value(serde_json::to_value(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}
impl Service {
    pub fn status(&self, provider: &str) -> Result<Status, String> {
        match provider {
            "jira" => convert(jira::jira_status(self.data_dir.clone())?),
            "gitlab" => convert(gitlab::gitlab_status(self.data_dir.clone())?),
            "github" => {
                let installed = crate::scm::resolve_gui_binary("gh").is_some();
                let authenticated = installed
                    && run_gh(
                        &self.cwd,
                        &["auth", "status", "--active", "--hostname", "github.com"],
                        true,
                    )
                    .is_ok();
                Ok(Status {
                    installed,
                    authenticated,
                    connected: authenticated,
                    ..Default::default()
                })
            }
            _ => Err("Unsupported inbox provider".into()),
        }
    }
    pub fn connect(
        &self,
        provider: &str,
        url: String,
        email: String,
        token: String,
    ) -> Result<Status, String> {
        match provider {
            "jira" => convert(jira::jira_set_config(
                self.data_dir.clone(),
                url,
                email,
                token,
            )?),
            "gitlab" => convert(gitlab::gitlab_set_config(
                self.data_dir.clone(),
                url,
                token,
            )?),
            "github" => self.status("github"),
            _ => Err("Unsupported inbox provider".into()),
        }
    }
    pub fn list(&self, projects: &[PathBuf], state: &ViewState) -> Snapshot {
        let mut snapshot = Snapshot::default();
        let limit = if state.filters.fetch_state() == "all" {
            100
        } else {
            40
        };
        for provider in state.connectors.enabled_providers() {
            let status = match self.status(provider) {
                Ok(status) => status,
                Err(error) => {
                    snapshot.errors.insert(provider.into(), error);
                    continue;
                }
            };
            let connected = status.connected;
            snapshot.connections.insert(provider.into(), status);
            if !connected {
                continue;
            }
            let result: Result<Vec<Item>, String> = (|| {
                if provider == "jira" {
                    if let Some(jql) = state.jira_filter.custom_jql() {
                        return convert(jira::jira_list_issues(
                            self.data_dir.clone(),
                            false,
                            "all".into(),
                            vec![],
                            Some(100),
                            Some(jql.into()),
                        )?);
                    }
                    let projects = jira::jira_list_projects(self.data_dir.clone())?;
                    let visible = projects
                        .iter()
                        .filter(|p| !state.hidden_jira_projects.contains(&p.id))
                        .map(|p| p.id.clone())
                        .collect::<Vec<_>>();
                    snapshot.jira_projects = convert(projects)?;
                    if !state.hidden_jira_projects.is_empty() && visible.is_empty() {
                        return Ok(vec![]);
                    }
                    return convert(jira::jira_list_issues(
                        self.data_dir.clone(),
                        state.filters.assigned_to_me,
                        state.filters.fetch_state().into(),
                        if state.hidden_jira_projects.is_empty() {
                            vec![]
                        } else {
                            visible
                        },
                        Some(limit),
                        None,
                    )?);
                }
                let mut items = Vec::new();
                let mut visited = HashSet::new();
                if state.filters.assigned_to_me && provider == "gitlab" {
                    for kind in ["issue", "pr"] {
                        let rows: Vec<Item> = convert(gitlab::gitlab_list_todos(
                            self.data_dir.clone(),
                            kind.into(),
                            Some(limit),
                        )?)?;
                        items.extend(rows.into_iter().map(|mut item| {
                            item.provider = provider.into();
                            item
                        }));
                    }
                    return Ok(items);
                }
                for project in projects {
                    let repos = match provider {
                        "github" => github::git_github_repositories_for(project),
                        "gitlab" => gitlab::gitlab_repo(
                            self.data_dir.clone(),
                            project.to_string_lossy().into_owned(),
                        )
                        .map(|r| vec![r]),
                        _ => Err("Unsupported inbox provider".into()),
                    };
                    let Ok(repos) = repos else {
                        continue;
                    };
                    for repo in repos {
                        if !visited.insert(repo.to_lowercase()) {
                            continue;
                        }
                        for kind in ["issue", "pr"] {
                            let rows: Vec<Item> = match provider {
                                "github" => convert(github::git_github_work_items_for(
                                    project,
                                    &repo,
                                    kind,
                                    state.filters.assigned_to_me,
                                    state.filters.fetch_state(),
                                    "",
                                    limit,
                                )?)?,
                                "gitlab" => convert(gitlab::gitlab_list_work_items(
                                    self.data_dir.clone(),
                                    project.to_string_lossy().into_owned(),
                                    kind.into(),
                                    false,
                                    state.filters.fetch_state().into(),
                                    Some(limit),
                                )?)?,
                                _ => return Err("Unsupported inbox provider".into()),
                            };
                            items.extend(rows.into_iter().map(|mut item| {
                                item.provider = provider.into();
                                item.project_path = project.to_string_lossy().into_owned();
                                item
                            }));
                        }
                    }
                }
                Ok(items)
            })();
            match result {
                Ok(items) => snapshot.items.extend(items),
                Err(error) => {
                    snapshot.errors.insert(provider.into(), error);
                }
            }
        }
        snapshot.items.sort_by(|a, b| {
            b.updated_ms()
                .cmp(&a.updated_ms())
                .then_with(|| a.title.cmp(&b.title))
        });
        let mut seen = HashSet::new();
        snapshot.items.retain(|i| seen.insert(i.key()));
        snapshot
    }
    pub fn details(&self, item: &Item) -> Result<Details, String> {
        match item.provider.as_str() {
            "jira" => convert(jira::jira_issue_details(
                self.data_dir.clone(),
                item.identifier.clone(),
            )?),
            "gitlab" => convert(gitlab::gitlab_work_item_details(
                self.data_dir.clone(),
                item.repo.clone(),
                item.kind.clone(),
                item.number,
            )?),
            "github" => convert(github::git_github_work_item_details_for(
                self.project(item),
                &item.repo,
                &item.kind,
                item.number,
            )?),
            _ => Err("Unsupported inbox provider".into()),
        }
    }
    fn project<'a>(&'a self, item: &'a Item) -> &'a Path {
        if item.project_path.is_empty() {
            &self.cwd
        } else {
            Path::new(&item.project_path)
        }
    }
    pub fn thread(&self, item: &Item) -> Result<Thread, String> {
        match item.provider.as_str() {
            "jira" => convert(jira::jira_issue_thread(
                self.data_dir.clone(),
                item.identifier.clone(),
            )?),
            "gitlab" => convert(gitlab::gitlab_work_item_thread(
                self.data_dir.clone(),
                item.repo.clone(),
                item.kind.clone(),
                item.number,
            )?),
            "github" => convert(github::git_github_work_item_thread_for(
                self.project(item),
                &item.repo,
                &item.kind,
                item.number,
            )?),
            _ => Err("Unsupported inbox provider".into()),
        }
    }
    pub fn comment(
        &self,
        item: &Item,
        body: String,
        reply_to: String,
    ) -> Result<CommentReceipt, String> {
        match item.provider.as_str() {
            "jira" => {
                jira::jira_issue_comment(self.data_dir.clone(), item.identifier.clone(), body)
            }
            "gitlab" => gitlab::gitlab_work_item_comment(
                self.data_dir.clone(),
                item.repo.clone(),
                item.kind.clone(),
                item.number,
                body,
            ),
            "github" => github::git_github_work_item_comment_for(
                self.project(item),
                &item.repo,
                &item.kind,
                item.number,
                &body,
                &reply_to,
            ),
            _ => Err("Unsupported inbox provider".into()),
        }
    }
    pub fn diff(&self, item: &Item) -> Result<Diff, String> {
        match item.provider.as_str() {
            "gitlab" => convert(gitlab::gitlab_mr_diff(
                self.data_dir.clone(),
                item.repo.clone(),
                item.number,
            )?),
            "github" => convert(github::git_github_pr_diff_for(
                self.project(item),
                &item.repo,
                item.number,
            )?),
            _ => Err("This issue has no pull request diff".into()),
        }
    }
    pub fn checks(&self, item: &Item) -> Result<Checks, String> {
        if item.provider != "github" {
            return Err("This action requires a GitHub item".into());
        }
        convert(github::git_github_pr_checks_for(
            self.project(item),
            &item.repo,
            item.number,
        )?)
    }
    pub fn action(&self, item: &Item, action: &str) -> Result<Item, String> {
        if item.provider != "github" {
            return Err("This action requires a GitHub item".into());
        }
        let mut updated: Item = convert(github::git_github_pr_action_for(
            self.project(item),
            &item.repo,
            item.number,
            action,
        )?)?;
        updated.provider = item.provider.clone();
        updated.project_path = item.project_path.clone();
        Ok(updated)
    }
}
pub(super) fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    run_command("git", root, args, true)
}
pub(super) fn run_gh(root: &Path, args: &[&str], allow_empty: bool) -> Result<String, String> {
    run_command("gh", root, args, allow_empty)
}
fn run_command(
    program: &str,
    root: &Path,
    args: &[&str],
    allow_empty: bool,
) -> Result<String, String> {
    let binary = crate::scm::resolve_gui_binary(program).ok_or_else(|| {
        format!(
            "{} is not installed. Install {} and check again.",
            if program == "gh" {
                "GitHub CLI (`gh`)"
            } else {
                program
            },
            program
        )
    })?;
    let mut command = Command::new(binary);
    command
        .current_dir(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("GIT_PAGER", "cat");
    crate::scm::shell::apply_gui_env(&mut command);
    let output = bounded::output(&mut command)?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
    if stdout.is_empty() && !allow_empty {
        Err(format!("{program} returned no output"))
    } else {
        Ok(stdout)
    }
}

#[cfg(test)]
mod notification_tests {
    use super::*;

    fn item(number: i64, provider: &str) -> Item {
        Item {
            provider: provider.into(),
            kind: "issue".into(),
            repo: "acme/app".into(),
            number,
            id: number.to_string(),
            updated_at: "2026-10-21T10:00:00Z".into(),
            ..Item::default()
        }
    }
    fn snapshot(items: Vec<Item>) -> Snapshot {
        Snapshot {
            items,
            ..Snapshot::default()
        }
    }

    #[test]
    fn first_baseline_and_existing_revisions_never_appear() {
        let mut tracker = NotificationTracker::default();
        assert!(tracker
            .observe(&snapshot(vec![item(1, "github")]), "scope".into())
            .is_empty());
        let mut updated = item(1, "github");
        updated.updated_at = "2026-10-21T11:00:00Z".into();
        let appeared = tracker.observe(&snapshot(vec![updated, item(2, "github")]), "scope".into());
        assert_eq!(
            appeared.iter().map(|i| i.number).collect::<Vec<_>>(),
            vec![2]
        );
        assert!(tracker
            .observe(&snapshot(vec![item(2, "github")]), "scope".into())
            .is_empty());
    }

    #[test]
    fn changed_revisions_are_distinct_from_new_items_and_never_replay_on_query_switch() {
        let mut tracker = NotificationTracker::default();
        let first = snapshot(vec![item(1, "jira")]);
        let baseline = tracker.observe_snapshot(&first, "project".into());
        assert!(baseline.changed.is_empty() && baseline.appeared.is_empty());
        let mut revised = item(1, "jira");
        revised.updated_at = "2026-10-23T10:00:00Z".into();
        let observation = tracker.observe_snapshot(
            &snapshot(vec![revised.clone(), item(2, "jira")]),
            "project".into(),
        );
        assert_eq!(observation.changed.len(), 2);
        assert_eq!(observation.appeared.len(), 1);
        let same = snapshot(vec![revised, item(2, "jira")]);
        assert!(tracker
            .observe_snapshot(&same, "project".into())
            .changed
            .is_empty());
        assert!(tracker
            .observe_snapshot(&same, "jql-filter".into())
            .changed
            .is_empty());
    }

    #[test]
    fn failed_provider_has_its_own_successful_baseline() {
        let mut tracker = NotificationTracker::default();
        let mut failed = snapshot(vec![item(1, "jira")]);
        failed.errors.insert("jira".into(), "offline".into());
        assert!(tracker.observe(&failed, "scope".into()).is_empty());
        assert!(tracker
            .observe(&snapshot(vec![item(2, "jira")]), "scope".into())
            .is_empty());
        let appeared = tracker.observe(
            &snapshot(vec![item(2, "jira"), item(3, "jira")]),
            "scope".into(),
        );
        assert_eq!(appeared.len(), 1);
        assert_eq!(appeared[0].number, 3);
    }

    #[test]
    fn changing_query_baselines_new_scope_without_forgetting_known_items() {
        let mut tracker = NotificationTracker::default();
        assert!(tracker
            .observe(&snapshot(vec![item(1, "github")]), "one".into())
            .is_empty());
        assert!(tracker
            .observe(&snapshot(vec![item(2, "github")]), "two".into())
            .is_empty());
        assert!(tracker
            .observe(
                &snapshot(vec![item(1, "github"), item(2, "github")]),
                "two".into()
            )
            .is_empty());
        assert_eq!(
            tracker
                .observe(&snapshot(vec![item(3, "github")]), "two".into())
                .len(),
            1
        );
    }

    #[test]
    fn seen_keys_match_desktop_identity_rules() {
        let mut github = item(12, "github");
        github.url = "https://GitHub.com/Acme/App/pull/12/".into();
        assert_eq!(github.key(), "github:acme/app:issue:12");
        let mut jira = item(7, "jira");
        jira.id = "ENG-7".into();
        jira.identifier = "ENG-7".into();
        assert_eq!(jira.key(), "jira:eng-7");
    }
}
