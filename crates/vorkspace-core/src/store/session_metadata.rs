//! Small sidebar metadata can be gathered before the UI constructs chat entities.
use super::Store;
use crate::{agent::AccessMode, orchestration, session_groups};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Default)]
pub struct SessionSidebarMetadata {
    pub worktrees: HashMap<i64, Option<String>>,
    pub details: HashMap<i64, SessionMetadata>,
    pub drafts: HashSet<i64>,
    pub sidebar_hidden: HashSet<i64>,
    pub started_by: HashMap<i64, i64>,
    pub orchestrations: HashMap<i64, orchestration::Summary>,
    pub groups: HashMap<String, session_groups::Groups>,
}

/// Scalar sidebar fields, including sessions with no live conversation view.
/// Model IDs use the native representation, as agent_settings does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionMetadata {
    pub model: Option<String>,
    pub branch: Option<String>,
    pub worktree_cwd: Option<String>,
    pub worktree_removed: bool,
    pub access: AccessMode,
}

impl Store {
    /// One bounded metadata query per publication. Never decode transcript
    /// bodies or model controls, or load a Conversation just to list it.
    pub(crate) fn session_metadata(&self) -> Result<HashMap<i64, SessionMetadata>, String> {
        let query = {
            format!(
                "SELECT id,harness,substr(model,1,512),substr({},1,512),{},coalesce({},0),coalesce({},'supervised') FROM sessions",
                self.shared_field("branch", "NULL"),
                self.shared_field("worktree_cwd", "NULL"),
                self.shared_field("worktree_removed", "0"),
                self.shared_field("runtime_mode", "'supervised'"),
            )
        };
        let mut statement = self.conn.prepare(&query).map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut result = HashMap::new();
        for row in rows {
            let (id, provider, model, branch, worktree_cwd, worktree_removed, access) =
                row.map_err(|e| e.to_string())?;
            let id = self.shared_register(&id)?;
            result.insert(
                id,
                SessionMetadata {
                    model: model
                        .as_deref()
                        .and_then(|model| super::native_model(model, &provider)),
                    branch,
                    worktree_cwd,
                    worktree_removed,
                    access: AccessMode::from_id(&access),
                },
            );
        }
        Ok(result)
    }

    /// Reads no transcript bodies or full agent settings. Call this on a loader
    /// thread, before handing the snapshot to the interactive shell.
    pub fn session_sidebar_metadata(&self, projects: &[String]) -> SessionSidebarMetadata {
        let details = self.session_metadata();
        let worktrees = match &details {
            Ok(details) => details
                .iter()
                .map(|(&id, metadata)| (id, metadata.worktree_cwd.clone()))
                .collect(),
            Err(_) => self.session_worktrees().unwrap_or_default(),
        };
        SessionSidebarMetadata {
            worktrees,
            details: details.unwrap_or_default(),
            drafts: self.draft_sessions().unwrap_or_default(),
            sidebar_hidden: { self.shared_sidebar_hidden().unwrap_or_default() },
            started_by: self.started_links().unwrap_or_default(),
            orchestrations: self.orchestration_summaries().unwrap_or_default(),
            groups: projects
                .iter()
                .filter_map(|project| {
                    self.groups(project)
                        .ok()
                        .map(|groups| (project.clone(), groups))
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn session_metadata_keeps_saved_branches_worktrees_models_and_access_without_decoding_history()
    {
        let store = Store::test(Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/project").unwrap();
        let id = store.persisted_session_id(session.id).unwrap();
        store.conn.execute("UPDATE sessions SET model='claude:opus-5-5[effort=high]',branch='feature/sidebar',worktree_cwd='/project-worktrees/sidebar',runtime_mode='full-access',model_settings='broken JSON',blocks_json='broken JSON' WHERE id=?1", [&id]).unwrap();
        let metadata = store
            .session_metadata()
            .unwrap()
            .remove(&session.id)
            .unwrap();
        assert_eq!(metadata.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(metadata.branch.as_deref(), Some("feature/sidebar"));
        assert_eq!(
            metadata.worktree_cwd.as_deref(),
            Some("/project-worktrees/sidebar")
        );
        assert_eq!(metadata.access, AccessMode::FullAccess);
        assert!(!metadata.worktree_removed);
        // Scalar updates need not change updated_at to appear in the sidebar.
        store.conn.execute("UPDATE sessions SET model='claude:sonnet',branch='main',worktree_removed=1,runtime_mode='read-only' WHERE id=?1", [&id]).unwrap();
        let next = store
            .session_metadata()
            .unwrap()
            .remove(&session.id)
            .unwrap();
        assert_eq!(next.model.as_deref(), Some("sonnet"));
        assert_eq!(next.branch.as_deref(), Some("main"));
        assert!(next.worktree_removed);
        assert_eq!(next.access, AccessMode::ReadOnly);
    }

    #[test]
    fn session_metadata_supports_legacy_defaults_and_invalid_settings_without_history_reads() {
        let store = Store::test(Path::new(":memory:")).unwrap();
        let first = store.create("codex", "/project").unwrap();
        let second = store.create("claude", "/other").unwrap();
        store
            .save_agent_preferences(
                first.id,
                &crate::agent::Settings {
                    model: Some("gpt-6.1-sol".into()),
                    worktree_cwd: Some("/linked".into()),
                    access: AccessMode::Auto,
                    worktree_removed: true,
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO native_agent_settings(session_id,state_json) VALUES(?1,'broken JSON')",
                [store.persisted_session_id(second.id).unwrap()],
            )
            .unwrap();
        store
            .conn
            .execute_batch(
                "UPDATE sessions SET blocks_json='broken JSON', model_settings='broken JSON'",
            )
            .unwrap();
        let metadata = store.session_metadata().unwrap();
        assert_eq!(metadata[&first.id].model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(metadata[&first.id].worktree_cwd.as_deref(), Some("/linked"));
        assert_eq!(metadata[&first.id].access, AccessMode::Auto);
        assert!(metadata[&first.id].worktree_removed);
        assert_eq!(metadata[&second.id], SessionMetadata::default());
    }

    #[test]
    fn sidebar_snapshot_keeps_old_history_without_reading_transcripts() {
        let store = Store::test(Path::new(":memory:")).unwrap();
        let first = store.create("codex", "/work/first").unwrap();
        let second = store.create("claude", "/work/second").unwrap();
        // A malformed body must not be decoded while gathering sidebar state.
        store
            .conn
            .execute(
                "UPDATE sessions SET blocks_json='not JSON',has_user_message=1 WHERE id=?1",
                [store.persisted_session_id(first.id).unwrap()],
            )
            .unwrap();
        let snapshot = store.session_sidebar_metadata(&["/work/first".into()]);
        assert!(snapshot.worktrees.contains_key(&first.id));
        assert!(snapshot.worktrees.contains_key(&second.id));
        assert!(snapshot.details.contains_key(&first.id));
        assert!(snapshot.details.contains_key(&second.id));
        assert!(snapshot.groups.contains_key("/work/first"));
        assert!(!snapshot.groups.contains_key("/work/second"));
    }
}
