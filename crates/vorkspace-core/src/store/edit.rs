//! Two-phase edit: capture a revision, then trim only after provider confirmation.
use super::*;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug)]
pub struct EditSnapshot {
    pub start: usize,
    pub prompt: String,
    pub attachments: Vec<PathBuf>,
    pub provider_turn_id: Option<String>,
    pub provider_id: String,
    pub account_id: String,
    pub cwd: String,
    pub rows: Vec<Row>,
    pub(super) blocks: Vec<Value>,
    pub(super) block_start: usize,
    pub(super) provider: String,
}
impl EditSnapshot {
    pub fn provider(&self) -> &str {
        &self.provider
    }
    pub fn matches(&self, rows: &[Row]) -> bool {
        self.rows == rows
    }
}

pub(super) fn codex_turn_start(
    rows: &[Row],
    settings: &crate::agent::Settings,
    id: &str,
) -> Option<usize> {
    let response = settings
        .turns
        .iter()
        .filter(|(_, info)| info.provider_turn_id.as_deref() == Some(id))
        .map(|(&row, _)| row)
        .min()?;
    rows.iter()
        .enumerate()
        .take(response)
        .rev()
        .find(|(_, row)| row.role == "user" && row.first)
        .map(|(row, _)| row)
}

/// Native history previously rendered files as trailing local Markdown links,
/// without retaining the attachment payload. Re-sending that text alone would
/// silently drop images. Refuse ambiguous local-file tails until a manifest is
/// available; rich desktop attachment lists are restored separately.
pub(super) fn untracked_attachment_tail(prompt: &str) -> bool {
    let Some(tail) = prompt.rsplit("\n\n").next() else {
        return false;
    };
    let Some(path) = tail
        .strip_prefix('[')
        .and_then(|tail| tail.split_once("](<"))
        .and_then(|(_, path)| path.strip_suffix(">)"))
    else {
        return false;
    };
    Path::new(path).is_absolute()
        || path.starts_with('/')
        || path.starts_with("\\\\")
        || path.as_bytes().get(1) == Some(&b':')
}

impl Store {
    pub(super) fn edit_preferences(&self, id: i64) -> Result<crate::agent::Settings, String> {
        let settings = self.agent_preferences(id)?;
        Ok(settings.unwrap_or_default())
    }

    /// Validate the captured revision inside admission, dispatch and commit.
    pub(super) fn validate_edit_last_turn(
        &self,
        id: i64,
        edit: &EditSnapshot,
    ) -> Result<(), String> {
        let session = self
            .bot_session(id)?
            .ok_or("Conversation no longer exists")?;
        if session.provider != edit.provider
            || session.provider_id.as_deref() != Some(edit.provider_id.as_str())
        {
            return Err("Provider session changed before the edit could be applied".into());
        }
        let settings = self.edit_preferences(id)?;
        if settings.account.id != edit.account_id
            || settings.worktree_cwd.as_deref().unwrap_or(&session.cwd) != edit.cwd
        {
            return Err(
                "The provider account or working directory changed; cancel the edit before sending"
                    .into(),
            );
        }
        self.shared_validate_edit_rows(id, &edit.blocks)
    }
}

#[cfg(test)]
pub(super) fn dispatched_edit(store: &Store, id: i64, edit: EditSnapshot) -> EditClaim {
    let claim = store
        .admit_edit_rewind(id, std::sync::Arc::new(edit))
        .unwrap();
    store.dispatch_edit_rewind(&claim).unwrap();
    claim
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Store, i64, Vec<Row>) {
        let mut store = Store::test(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        store.provider_id(session.id, "thread").unwrap();
        let mut transcript = Transcript::default();
        for (role, text) in [
            ("user", "Earlier"),
            ("assistant", "Kept"),
            ("user", "Edit me"),
            ("assistant", "Replaced"),
        ] {
            transcript.begin(role);
            transcript.append(text);
        }
        store.write_rows(session.id, 0, &transcript.rows).unwrap();
        let mut settings = crate::agent::Settings::default();
        settings.turns.insert(
            3,
            crate::agent::TurnInfo {
                provider_turn_id: Some("turn".into()),
                ..Default::default()
            },
        );
        store.save_agent_settings(session.id, &settings).unwrap();
        (store, session.id, transcript.rows)
    }
    #[test]
    fn preparing_and_cancelling_an_edit_leaves_all_rows_intact() {
        let (store, id, rows) = fixture();
        let edit = store
            .prepare_edit_last_turn(id, &rows, Some("turn"))
            .unwrap();
        assert_eq!(edit.start, 2);
        assert_eq!(edit.prompt, "Edit me");
        assert_eq!(store.load(id).unwrap().rows, rows);
    }
    #[test]
    fn confirmed_edit_truncates_exactly_the_last_turn_and_rebinds_forks() {
        let (store, id, rows) = fixture();
        let edit = store
            .prepare_edit_last_turn(id, &rows, Some("turn"))
            .unwrap();
        let mut claim = dispatched_edit(&store, id, edit);
        store.finish_edit_last_turn(&mut claim, "fork").unwrap();
        assert_eq!(store.load(id).unwrap().rows, rows[..2]);
        assert_eq!(
            store.sessions().unwrap()[0].provider_id.as_deref(),
            Some("fork")
        );
    }
    #[test]
    fn late_rows_or_provider_binding_changes_are_never_overwritten() {
        let (mut store, id, rows) = fixture();
        let edit = store
            .prepare_edit_last_turn(id, &rows, Some("turn"))
            .unwrap();
        let mut claim = dispatched_edit(&store, id, edit);
        store.provider_id(id, "another-thread").unwrap();
        assert!(store.finish_edit_last_turn(&mut claim, "fork").is_err());
        assert_eq!(store.load(id).unwrap().rows, rows);
        store.provider_id(id, "thread").unwrap();
        let mut changed = rows.clone();
        changed[3].text = "Another window".into();
        store.write_rows(id, 3, &changed[3..]).unwrap();
        assert!(store.finish_edit_last_turn(&mut claim, "fork").is_err());
        assert_eq!(store.load(id).unwrap().rows, changed);
    }
    #[test]
    fn codex_steered_messages_recall_only_latest_prompt_but_replace_the_complete_provider_turn() {
        let (mut store, id, mut rows) = fixture();
        let mut transcript = Transcript { rows };
        transcript.begin("user");
        transcript.append("Latest steer");
        transcript.begin("assistant");
        transcript.append("After steer");
        rows = transcript.rows;
        store.write_rows(id, 4, &rows[4..]).unwrap();
        let mut settings = store.agent_settings(id).unwrap().unwrap();
        settings.turns.insert(
            5,
            crate::agent::TurnInfo {
                provider_turn_id: Some("turn".into()),
                ..Default::default()
            },
        );
        store.save_agent_settings(id, &settings).unwrap();
        let edit = store
            .prepare_edit_last_turn(id, &rows, Some("turn"))
            .unwrap();
        assert_eq!(edit.prompt, "Latest steer");
        assert_eq!(edit.start, 2);
        let mut claim = dispatched_edit(&store, id, edit);
        store.finish_edit_last_turn(&mut claim, "thread").unwrap();
        assert_eq!(store.load(id).unwrap().rows, rows[..2]);
        assert!(store.agent_settings(id).unwrap().unwrap().turns.is_empty());
    }
    #[test]
    fn native_attachment_links_are_never_recalled_as_text_without_the_original_files() {
        for path in [
            "/tmp/photo.png",
            r"C:\Users\me\photo.png",
            "C:/Users/me/photo.png",
            r"\\server\share\photo.png",
        ] {
            let (mut store, id, mut rows) = fixture();
            rows[2].text = format!("Describe image\n\n[photo.png](<{path}>)");
            store.write_rows(id, 2, &rows[2..]).unwrap();
            assert!(store
                .prepare_edit_last_turn(id, &rows, Some("turn"))
                .unwrap_err()
                .contains("attachment manifest"));
            assert_eq!(store.load(id).unwrap().rows, rows);
        }
        for link in [
            "A [web link](https://example.com)",
            "[photo.png](<https://example.com/photo.png>)",
            "[photo.png](<relative/photo.png>)",
        ] {
            assert!(!untracked_attachment_tail(link));
        }
    }
    #[test]
    fn provider_confirmed_conflict_leaves_a_durable_recovery_guard_and_blocks_recall() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.db");
        let mut store = Store::test(&path).unwrap();
        let session = store.create("pi", "/project").unwrap();
        store.provider_id(session.id, "session").unwrap();
        let mut transcript = Transcript::default();
        transcript.begin("user");
        transcript.append("Prompt");
        store.write_rows(session.id, 0, &transcript.rows).unwrap();
        let edit = store
            .prepare_edit_last_turn(session.id, &transcript.rows, None)
            .unwrap();
        let mut claim = dispatched_edit(&store, session.id, edit);
        store.provider_id(session.id, "another-session").unwrap();
        assert!(store.finish_edit_last_turn(&mut claim, "fork").is_err());
        drop(claim);
        assert_eq!(
            store.edit_barrier(session.id).unwrap(),
            Some(EditBarrier::RecoveryRequired)
        );
        drop(store);
        let reopened = Store::test(&path).unwrap();
        reopened.recover_local_submissions().unwrap();
        assert_eq!(
            reopened.edit_barrier(session.id).unwrap(),
            Some(EditBarrier::RecoveryRequired)
        );
        assert!(reopened
            .prepare_edit_last_turn(session.id, &transcript.rows, None)
            .is_err());
        assert_eq!(reopened.load(session.id).unwrap().rows, transcript.rows);
    }
}
