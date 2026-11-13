//! Session storage. Desktop launches use vorkspace's existing schema directly.
mod agent_configuration;
pub use agent_configuration::ProviderSource;
mod app_commands;
pub use app_commands::AppCommandAdmission;
mod attachments;
pub(crate) mod bot_state;
pub use bot_state::describe_worktrees;
mod bots;
pub(crate) mod inbox_preferences;
pub use bot_state::root as bot_root;
mod composer_draft;
mod follow_up;
pub use composer_draft::{ComposerDraft, ComposerOrigin, ComposerRecovery};
mod local_submissions;
pub use local_submissions::{
    EditAdmissionError, EditBarrier, EditClaim, SubmissionClaim, SubmissionInput, SubmissionOwner,
    SubmissionReceipt, SubmissionReservation,
};
mod managed_sessions;
mod notes;
pub use notes::NoteRecording;
mod peer_history;
mod session_folders;
mod session_metadata;
pub use session_metadata::{SessionMetadata, SessionSidebarMetadata};
mod session_preview;
pub use session_preview::SessionPreviewData;
mod edit;
mod shared;
pub use shared::{IndexedHistoryCursor, IndexedHistoryPage};
mod native_history;
mod side_threads;
mod turn_history;
pub use native_history::{NativeHistoryRequest, NativeHistoryResult};
pub use turn_history::{recovery_text, RecoveredTurn, TurnHistory};
pub(crate) mod todos;
pub use crate::data_location::vorkspace_database;
pub use crate::session::native_model;
use crate::transcript::{Row, Transcript};
pub use edit::EditSnapshot;
use rusqlite::{params, Connection};
pub use side_threads::DesktopSideThread;
use std::path::Path;

pub use crate::session::Session;

/// A session's pending or delivered reminder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    pub session_id: i64,
    /// Unix milliseconds.
    pub due_at: u64,
    pub fired_at: Option<u64>,
}

/// Rows, historical turn metadata, and revision from one local read snapshot.
pub struct LoadedHistory {
    pub transcript: Transcript,
    /// A damaged preference record must still leave saved messages readable.
    pub settings: Result<Option<crate::agent::Settings>, String>,
    /// Binding from the same scalar observation, available when preferences decode.
    pub binding: Option<LocalAgentBinding>,
    pub revision: i64,
}

/// The local provider thread observed with its conversation preferences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAgentBinding {
    pub provider: String,
    pub provider_id: Option<String>,
}

pub struct Store {
    conn: Connection,
    path: std::path::PathBuf,
    shared: shared::Shared,
    project_deletions: std::collections::HashSet<String>,
    #[cfg(any(test, feature = "test-support"))]
    test_directory: Option<std::sync::Arc<tempfile::TempDir>>,
}
fn validate_bot_claim(raw: &str, bot: &str, message: &crate::bot::Message) -> Result<(), String> {
    let roster: crate::bot::Roster =
        serde_json::from_str(raw).map_err(|e| format!("Cannot verify Bot outbox: {e}"))?;
    let queued = roster
        .get(bot)
        .and_then(|bot| bot.outbox.iter().find(|queued| queued.id == message.id));
    if queued
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| e.to_string())?
        != Some(serde_json::to_value(message).map_err(|e| e.to_string())?)
        || message.error.is_some()
    {
        return Err("Bot outbox changed in another window; reopen it before sending.".into());
    }
    Ok(())
}

impl Store {
    /// Runtime admission guard held until the project deletion publishes its result.
    pub fn begin_project_deletion(&mut self, path: &str) -> Result<(), String> {
        let key = crate::files::path_key(path);
        if !self.project_deletions.insert(key) {
            return Err("Project deletion is already in progress.".into());
        }
        Ok(())
    }
    pub fn end_project_deletion(&mut self, path: &str) {
        self.project_deletions.remove(&crate::files::path_key(path));
    }
    fn check_project_creation(&self, path: &str) -> Result<(), String> {
        let key = crate::files::path_key(path);
        if self.project_deletions.iter().any(|root| {
            std::path::Path::new(&key).starts_with(std::path::Path::new(root))
                || match (std::fs::canonicalize(&key), std::fs::canonicalize(root)) {
                    (Ok(path), Ok(root)) => path.starts_with(root),
                    _ => false,
                }
        }) {
            return Err("This project is being deleted; try again afterward.".into());
        }
        Ok(())
    }
    /// The selected desktop or isolated-development database.
    pub fn database_path(&self) -> &Path {
        &self.path
    }
    pub fn data_dir(&self) -> Option<&Path> {
        (self.path != Path::new(":memory:"))
            .then(|| self.path.parent())
            .flatten()
    }
    /// Only call when dispatch was rejected before any provider call.
    pub fn release_unsubmitted_bot_claim(
        &self,
        bot: &str,
        message: &str,
        token: &str,
    ) -> Result<(), String> {
        if !shared::has_table(&self.conn, "native_bot_dispatch_claims")? {
            return Ok(());
        }
        self.conn.execute("DELETE FROM native_bot_dispatch_claims WHERE bot_id=?1 AND message_id=?2 AND token=?3", params![bot,message,token]).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn save_context_usage(&self, id: i64, usage: &crate::context::Usage) -> Result<(), String> {
        self.shared_patch(
            id,
            &[
                ("context_used", serde_json::json!(usage.used)),
                ("context_window", serde_json::json!(usage.window)),
            ],
        )
    }
    pub fn save_note(&self, note: &crate::response::Note) -> Result<(), String> {
        self.save_response_note(note)
    }
    pub fn response_note(&self, note: crate::notes::Note) -> crate::response::Note {
        let source_session = self.note_source_session(&note).unwrap_or(0);
        crate::response::Note {
            id: crate::notes::numeric_id(&note.id),
            title: note.title,
            body: note.body,
            tags: note.tags,
            source_session,
            source_cwd: note.source_cwd.unwrap_or_default(),
        }
    }

    pub fn save_ui_state(&self, state: &str) -> Result<(), String> {
        self.update_settings(|_| Ok(((), vec![("workspace".into(), state.into())])))
    }
    pub fn sessions(&self) -> Result<Vec<Session>, String> {
        let mut sessions = self.read_sessions()?;
        sessions.sort_by_key(|session| {
            (
                std::cmp::Reverse(session.updated_at),
                std::cmp::Reverse(session.id),
            )
        });
        Ok(sessions)
    }

    /// Each session another one started through the app CLI, and its caller.
    pub(crate) fn started_links(&self) -> Result<std::collections::HashMap<i64, i64>, String> {
        let mut links = self.shared_started_links()?;
        let saved: std::collections::HashMap<_, _> = self
            .settings_with_prefix("started-by:")?
            .into_iter()
            .map(|(child, value)| Ok((child, value.trim().parse().map_err(|_| "Invalid caller")?)))
            .collect::<Result<_, String>>()?;
        links.extend(saved);
        Ok(links)
    }
    pub fn save_started_link(&self, child: i64, caller: i64) -> Result<(), String> {
        self.save_setting(&format!("started-by:{child}"), &caller.to_string())
    }
    /// Every lead's last orchestration summary.
    pub(crate) fn orchestration_summaries(
        &self,
    ) -> Result<std::collections::HashMap<i64, crate::orchestration::Summary>, String> {
        Ok(self
            .settings_with_prefix("orchestration:")?
            .into_iter()
            .filter_map(|(lead, value)| Some((lead, serde_json::from_str(&value).ok()?)))
            .collect())
    }
    pub fn save_orchestration_summary(
        &self,
        lead: i64,
        summary: &crate::orchestration::Summary,
    ) -> Result<(), String> {
        let value = serde_json::to_string(summary).map_err(|e| e.to_string())?;
        self.save_setting(&format!("orchestration:{lead}"), &value)
    }
    pub fn session_history_revision(&self, session: i64) -> Result<i64, String> {
        let original = self.persisted_session_id(session)?;
        self.history_revision(&original)
    }

    pub fn load_local_history(&self, session: i64) -> Result<LoadedHistory, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        let revision = self.session_history_revision(session)?;
        let (binding, mut settings) = {
            match self.shared_agent_configuration(session) {
                Ok(Some((binding, settings))) => (Some(binding), Ok(Some(settings))),
                Ok(None) => (None, Ok(None)),
                Err(error) => (None, Err(error)),
            }
        };
        let transcript = self.shared_load_projected(
            session,
            settings
                .as_mut()
                .ok()
                .and_then(Option::as_mut)
                .map(|s| &mut s.turns),
        )?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(LoadedHistory {
            transcript,
            settings,
            binding,
            revision,
        })
    }

    pub fn write_rows(&mut self, session: i64, start: usize, rows: &[Row]) -> Result<(), String> {
        self.write_rows_with_message(session, start, rows, None)
    }
    pub fn write_rows_with_message(
        &mut self,
        session: i64,
        start: usize,
        rows: &[Row],
        message_id: Option<&str>,
    ) -> Result<(), String> {
        self.shared_write_rows(session, start, rows, message_id, None)
            .map(|_| ())
    }
    /// Archiving preserves the transcript, provider context and configuration.
    pub fn set_archived(&self, id: i64, archived: bool) -> Result<(), String> {
        self.shared_patch(id, &[("archived", serde_json::json!(archived))])
    }
    pub fn set_work_item(
        &self,
        id: i64,
        item: Option<&crate::session_groups::WorkItem>,
    ) -> Result<(), String> {
        self.shared_patch(
            id,
            &[(
                "linked_work_item_json",
                item.map(shared::encode)
                    .transpose()?
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
            )],
        )
    }
    /// Deletes the conversation, its settings and reminder for good.
    pub fn delete(&mut self, id: i64) -> Result<(), String> {
        self.delete_checked(id, None)
    }
    pub fn provider_id(&self, id: i64, provider_id: &str) -> Result<(), String> {
        self.shared_patch(
            id,
            &[("provider_session_id", serde_json::json!(provider_id))],
        )
    }
    /// Keep conversation history while detaching a removed working copy.
    pub fn detach_worktree(
        &self,
        path: &str,
        main: &str,
    ) -> Result<Vec<(Session, crate::agent::Settings)>, String> {
        self.detach_worktree_paths(&[path], main)
    }

    /// Resolve stored aliases before removal makes symlinks into the checkout
    /// unreadable. The returned spellings can be matched after Git removes it.
    pub fn worktree_paths_for_removal(&self, path: &str) -> Result<Vec<String>, String> {
        let worktrees = self.session_worktrees()?;
        let mut paths = std::collections::HashSet::from([path.to_owned()]);
        for session in self.sessions()? {
            for candidate in std::iter::once(session.cwd.as_str())
                .chain(worktrees.get(&session.id).and_then(Option::as_deref))
            {
                if !crate::files::is_equal_or_inside(candidate, path)
                    && crate::files::is_resolved_inside(candidate, path)
                {
                    paths.insert(candidate.to_owned());
                }
            }
        }
        Ok(paths.into_iter().collect())
    }

    pub fn reset_provider_id(&self, id: i64) -> Result<(), String> {
        self.shared_patch(id, &[("provider_session_id", serde_json::Value::Null)])
    }
    pub fn title(&self, id: i64, title: &str) -> Result<(), String> {
        self.shared_patch(id, &[("title", serde_json::json!(title))])
    }
}

#[cfg(test)]
mod tests;
