//! Project work and personal to-dos. Legacy Bot board keys remain readable so
//! their saved work can be assigned to a project without losing it.
use serde::{Deserialize, Serialize};
pub mod github;

pub const MAX_TITLE: usize = 500;
pub(crate) const MAX_DESCRIPTION: usize = 256 * 1024;
pub(crate) const MAX_ATTACHMENTS: usize = 50;
pub(crate) const MAX_DEPENDENCIES: usize = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TodoStatus {
    #[default]
    Todo,
    InProgress,
    InReview,
    Done,
}

impl TodoStatus {
    pub const ALL: [Self; 4] = [Self::Todo, Self::InProgress, Self::InReview, Self::Done];
    pub fn id(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "inProgress",
            Self::InReview => "inReview",
            Self::Done => "done",
        }
    }
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.id() == id)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Todo => "To do",
            Self::InProgress => "In progress",
            Self::InReview => "In review",
            Self::Done => "Done",
        }
    }
    /// The next status a single click moves an item to.
    pub fn next(self) -> Self {
        match self {
            Self::Todo => Self::InProgress,
            Self::InProgress => Self::InReview,
            Self::InReview => Self::Done,
            Self::Done => Self::Todo,
        }
    }
}

/// Which board an item belongs to, stored as its `key`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum TodoBoard {
    Personal,
    /// A project's path.
    Project(String),
    /// A Bot's roster id.
    Bot(String),
}

impl TodoBoard {
    pub fn key(&self) -> String {
        match self {
            Self::Personal => "personal".into(),
            Self::Project(path) => format!("project:{path}"),
            Self::Bot(id) => format!("bot:{id}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Project-relative file paths or HTTPS attachment URLs.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Local to-do IDs, GitHub issue numbers (`#123`), or GitHub issue URLs.
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub status: TodoStatus,
    /// Order within its column, ascending.
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub issue: Option<TodoIssue>,
    #[serde(default)]
    pub sync: TodoSync,
    #[serde(default)]
    pub pending_edit: Option<TodoRemoteEdit>,
    /// Private reconciliation marker; never returned over app JSON.
    #[serde(default, skip_serializing)]
    pub submission_token: Option<String>,
    /// Repository selected when publication began, even if origin changes.
    #[serde(default)]
    pub pending_repo: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoIssue {
    pub repo: String,
    pub number: i64,
    pub url: String,
    #[serde(default)]
    pub labels: Vec<String>,
    /// The provider returned only part of this issue's dependency list.
    #[serde(default)]
    pub dependencies_truncated: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TodoSync {
    #[default]
    Local,
    Pending,
    Uncertain,
    Synced,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoRemoteEdit {
    /// Identity of the admitted attempt, retained across recovery reads.
    #[serde(default, skip_serializing)]
    pub attempt_id: String,
    pub input: serde_json::Value,
    pub phase: TodoSync,
}

/// Trims and bounds a title; empty titles are rejected.
pub fn clean_title(title: &str) -> Result<String, String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return Err("Provide a title".into());
    }
    if title.len() > MAX_TITLE {
        return Err(format!("Titles are limited to {MAX_TITLE} bytes"));
    }
    Ok(title)
}

pub(crate) fn clean_description(description: &str) -> Result<String, String> {
    if description.len() > MAX_DESCRIPTION {
        return Err(format!(
            "Descriptions are limited to {MAX_DESCRIPTION} bytes"
        ));
    }
    Ok(description.trim().to_string())
}

pub(crate) fn clean_references(
    values: &[String],
    max: usize,
    kind: &str,
) -> Result<Vec<String>, String> {
    if values.len() > max {
        return Err(format!("Too many {kind} (limit {max})"));
    }
    let mut cleaned = Vec::with_capacity(values.len());
    for value in values {
        let value = value.trim();
        if value.is_empty()
            || value.len() > 4096
            || value.chars().any(|ch| matches!(ch, '\n' | '\r' | '\0'))
        {
            return Err(format!("Invalid {kind} reference"));
        }
        if !cleaned.iter().any(|old| old == value) {
            cleaned.push(value.to_string());
        }
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_cycle_and_titles_are_cleaned() {
        assert_eq!(TodoStatus::Todo.next(), TodoStatus::InProgress);
        assert_eq!(TodoStatus::Done.next(), TodoStatus::Todo);
        assert_eq!(
            TodoStatus::from_id("inProgress"),
            Some(TodoStatus::InProgress)
        );
        assert_eq!(clean_title("  ship \n it ").unwrap(), "ship it");
        assert!(clean_title("   ").is_err());
        assert!(clean_title(&"x".repeat(MAX_TITLE + 1)).is_err());
    }
}
