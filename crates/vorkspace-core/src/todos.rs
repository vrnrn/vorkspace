//! Very simple to-do boards: one per project, one per Bot and one personal
//! board, each with To do, In progress and Done columns.
use serde::{Deserialize, Serialize};

pub const MAX_TITLE: usize = 500;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TodoStatus {
    #[default]
    Todo,
    InProgress,
    Done,
}

impl TodoStatus {
    pub const ALL: [Self; 3] = [Self::Todo, Self::InProgress, Self::Done];
    pub fn id(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "inProgress",
            Self::Done => "done",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.id() == id)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Todo => "To do",
            Self::InProgress => "In progress",
            Self::Done => "Done",
        }
    }
    /// The next status a single click moves an item to.
    pub fn next(self) -> Self {
        match self {
            Self::Todo => Self::InProgress,
            Self::InProgress => Self::Done,
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
    pub fn from_key(key: &str) -> Option<Self> {
        if key == "personal" {
            return Some(Self::Personal);
        }
        if let Some(path) = key.strip_prefix("project:").filter(|p| !p.is_empty()) {
            return Some(Self::Project(path.into()));
        }
        key.strip_prefix("bot:")
            .filter(|id| !id.is_empty())
            .map(|id| Self::Bot(id.into()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub id: String,
    pub title: String,
    pub status: TodoStatus,
    /// Order within its column, ascending.
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_keys_round_trip() {
        for board in [
            TodoBoard::Personal,
            TodoBoard::Project("/work/a:b".into()),
            TodoBoard::Bot("abc".into()),
        ] {
            assert_eq!(TodoBoard::from_key(&board.key()), Some(board));
        }
        assert_eq!(TodoBoard::from_key("project:"), None);
        assert_eq!(TodoBoard::from_key("other"), None);
    }

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
