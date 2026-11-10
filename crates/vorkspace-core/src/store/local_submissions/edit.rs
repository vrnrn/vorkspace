//! Rewind claims share the provider admission lane with prompts and compaction.
use super::{dispatch, get, instance, pid_alive, settle, Store, SubmissionInput, SubmissionOwner};
use crate::store::{shared, EditSnapshot};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::json;
use std::sync::Arc;

pub(super) const ATTEMPT_PREFIX: &str = "edit-rewind-";
pub(super) const ATTEMPT_PATTERN: &str = "edit-rewind-*";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditBarrier {
    InFlight,
    RecoveryRequired,
}

impl EditBarrier {
    pub fn message(self) -> &'static str {
        match self {
            Self::InFlight => "A message edit is in progress in this conversation. Try again when it finishes.",
            Self::RecoveryRequired => "A previous edit may have changed provider context. Start a new conversation. The saved history and draft are preserved here.",
        }
    }
}

#[derive(Debug)]
pub enum EditAdmissionError {
    Changed(String),
    Unavailable(String),
}

pub struct EditClaim {
    pub(in crate::store) session: i64,
    pub(in crate::store) snapshot: Arc<EditSnapshot>,
    attempt: String,
    owner: Option<SubmissionOwner>,
}

impl EditClaim {
    pub(in crate::store) fn confirm(&self, conn: &Connection) -> Result<(), String> {
        if self.owner.is_none()
            || !get(conn, &self.attempt)?.is_some_and(|receipt| receipt.phase == "dispatching")
        {
            return Err("This message edit no longer owns provider dispatch".into());
        }
        settle(conn, &self.attempt, "completed", None)?;
        Ok(())
    }

    pub(in crate::store) fn close(&mut self) {
        self.owner = None;
    }
}

impl Drop for EditClaim {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            owner.release(&self.attempt);
        }
    }
}

impl Store {
    pub fn edit_barrier(&self, session: i64) -> Result<Option<EditBarrier>, String> {
        if shared::has_table(&self.conn, "local_submissions")? {
            let source = self.submission_source_id(session)?;
            let row = self
                .conn
                .query_row(
                    "SELECT attempt_id,phase,owner_pid,owner_instance FROM local_submissions
                 WHERE session_id=?1 AND attempt_id GLOB ?2 AND user_turn_json IS NULL
                 AND phase IN ('reserved','dispatching','uncertain')
                 ORDER BY (phase='uncertain') DESC,updated_at DESC,attempt_id LIMIT 1",
                    params![source, ATTEMPT_PATTERN],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, u32>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some((attempt, phase, pid, owner)) = row {
                if phase == "uncertain"
                    || (phase == "dispatching"
                        && (self.local_submission_owner().has_released(&attempt)
                            || (owner != instance() && !pid_alive(pid))))
                {
                    return Ok(Some(EditBarrier::RecoveryRequired));
                }
                return Ok(Some(EditBarrier::InFlight));
            }
        }
        // Preserve the recovery marker written by earlier builds.
        Ok(self
            .setting(&format!("edit-recovery:{session}"))?
            .filter(|reason| !reason.is_empty())
            .map(|_| EditBarrier::RecoveryRequired))
    }

    pub fn edit_recovery_reason(&self, session: i64) -> Result<Option<String>, String> {
        Ok(self
            .edit_barrier(session)?
            .map(|barrier| barrier.message().into()))
    }

    /// Reserve the exact source before touching the provider process. The
    /// immediate transaction also fences another window's send and native import.
    pub fn admit_edit_rewind(
        &self,
        session: i64,
        snapshot: Arc<EditSnapshot>,
    ) -> Result<EditClaim, EditAdmissionError> {
        let attempt = format!("{ATTEMPT_PREFIX}{}", uuid::Uuid::new_v4());
        let mut changed = false;
        let reservation = self.reserve_submission_checked(
            session,
            SubmissionInput {
                attempt_id: attempt.clone(),
                user_turn_id: attempt.clone(),
                session_id: String::new(),
                payload_json: json!({
                    "provider":snapshot.provider(),
                    "providerId":snapshot.provider_id,
                    "accountId":snapshot.account_id,
                    "cwd":snapshot.cwd,
                    "beforeTurn":snapshot.provider_turn_id,
                })
                .to_string(),
                user_turn: None,
            },
            || {
                if let Some(reason) = self.edit_recovery_reason(session)? {
                    return Err(reason);
                }
                self.validate_edit_last_turn(session, &snapshot)
                    .inspect_err(|_| changed = true)
            },
        );
        match reservation {
            Ok(reservation) if reservation.created => Ok(EditClaim {
                session,
                snapshot,
                attempt,
                owner: Some(self.local_submission_owner()),
            }),
            Ok(_) => Err(EditAdmissionError::Unavailable(
                "This edit was already admitted".into(),
            )),
            Err(reason) if changed => Err(EditAdmissionError::Changed(reason)),
            Err(reason) => Err(EditAdmissionError::Unavailable(reason)),
        }
    }

    /// Publish dispatch before the external mutation. Preparation may have
    /// waited on the provider while a peer changed local history or settings.
    pub fn dispatch_edit_rewind(&self, claim: &EditClaim) -> Result<(), String> {
        if claim.owner.is_none() {
            return Err("This edit has already finished".into());
        }
        let tx = self.edit_transaction()?;
        self.validate_edit_last_turn(claim.session, &claim.snapshot)?;
        if !dispatch(&tx, &claim.attempt)?.claimed {
            return Err("This edit no longer owns the conversation".into());
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn reject_edit_rewind(
        &self,
        claim: &mut EditClaim,
        error: String,
        uncertain: bool,
    ) -> Result<(), String> {
        settle(
            &self.conn,
            &claim.attempt,
            if uncertain { "uncertain" } else { "failed" },
            Some(error),
        )?;
        claim.close();
        Ok(())
    }

    /// Keep the winning local transcript after a confirmed rewind. The
    /// competing draft must already be durable before settling the receipt.
    pub fn recover_confirmed_rewind(
        &self,
        claim: &mut EditClaim,
        returned_binding: &str,
    ) -> Result<(), String> {
        let tx = self.edit_transaction()?;
        let id = claim.session;
        let edit = &claim.snapshot;
        if returned_binding == edit.provider_id {
            if let Some(session) = self.bot_session(id)? {
                if session.provider == edit.provider()
                    && session.provider_id.as_deref() == Some(returned_binding)
                    && self.edit_preferences(id)?.account.id == edit.account_id
                {
                    self.reset_provider_id_if(id, Some(returned_binding))?;
                }
            }
        }
        claim.confirm(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        claim.close();
        Ok(())
    }

    pub(in crate::store) fn edit_transaction(&self) -> Result<Transaction<'_>, String> {
        if self.shared.is_some() {
            self.shared_writable()?;
        }
        Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Transcript;

    fn fixture() -> (tempfile::TempDir, Store, i64, Arc<EditSnapshot>) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_vorkspace(&dir.path().join("native.db")).unwrap();
        let id = store.create("pi", "/project").unwrap().id;
        store.provider_id(id, "original").unwrap();
        let mut history = Transcript::default();
        for (role, text) in [
            ("user", "Earlier"),
            ("assistant", "Answer"),
            ("user", "Edit"),
        ] {
            history.begin(role);
            history.append(text);
        }
        store.write_rows(id, 0, &history.rows).unwrap();
        let snapshot = Arc::new(
            store
                .prepare_edit_last_turn(id, &history.rows, None)
                .unwrap(),
        );
        (dir, store, id, snapshot)
    }

    fn submission(attempt: &str, prompt: bool) -> SubmissionInput {
        let user_turn_id = format!("{attempt}-turn");
        SubmissionInput {
            attempt_id: attempt.into(),
            user_turn_id: user_turn_id.clone(),
            session_id: String::new(),
            payload_json: "{}".into(),
            user_turn: prompt.then(|| json!({"role":"user","id":user_turn_id,"text":"Peer"})),
        }
    }

    #[test]
    fn rewind_and_other_provider_operations_exclude_each_other_across_stores() {
        let (dir, store, id, snapshot) = fixture();
        let peer = Store::open_vorkspace(&dir.path().join("native.db")).unwrap();
        let mut edit = store.admit_edit_rewind(id, snapshot.clone()).unwrap();
        assert_eq!(peer.edit_barrier(id).unwrap(), Some(EditBarrier::InFlight));
        assert!(peer
            .reserve_local_submission(id, submission("prompt", true))
            .is_err());
        assert!(peer
            .reserve_local_submission(id, submission("compact", false))
            .is_err());
        assert!(peer.admit_edit_rewind(id, snapshot.clone()).is_err());
        assert!(store.finish_edit_last_turn(&mut edit, "fork").is_err());
        store.dispatch_edit_rewind(&edit).unwrap();
        store
            .reject_edit_rewind(&mut edit, "Provider rejected".into(), false)
            .unwrap();
        assert_eq!(peer.edit_barrier(id).unwrap(), None);
        assert_eq!(peer.load(id).unwrap().rows, snapshot.rows);

        peer.reserve_local_submission(id, submission("prompt", true))
            .unwrap();
        assert!(store.admit_edit_rewind(id, snapshot.clone()).is_err());
        peer.settle_local_submission("prompt", "failed", None)
            .unwrap();
        let mut edit = store.admit_edit_rewind(id, snapshot.clone()).unwrap();
        store.dispatch_edit_rewind(&edit).unwrap();
        store.finish_edit_last_turn(&mut edit, "fork").unwrap();
        assert_eq!(peer.edit_barrier(id).unwrap(), None);
        assert_eq!(
            peer.bot_session(id)
                .unwrap()
                .unwrap()
                .provider_id
                .as_deref(),
            Some("fork")
        );
        assert_eq!(peer.load(id).unwrap().rows, snapshot.rows[..snapshot.start]);
    }

    #[test]
    fn owner_loss_releases_an_undispatched_edit_and_fences_a_dispatched_edit() {
        let (dir, store, id, snapshot) = fixture();
        let path = dir.path().join("native.db");
        let peer = Store::open_vorkspace(&path).unwrap();
        drop(store.admit_edit_rewind(id, snapshot.clone()).unwrap());
        peer.recover_local_submissions().unwrap();
        assert_eq!(peer.edit_barrier(id).unwrap(), None);

        let edit = store.admit_edit_rewind(id, snapshot.clone()).unwrap();
        store.dispatch_edit_rewind(&edit).unwrap();
        drop(edit);
        assert_eq!(
            peer.edit_barrier(id).unwrap(),
            Some(EditBarrier::RecoveryRequired)
        );
        drop(store);
        drop(peer);

        let reopened = Store::open_vorkspace(&path).unwrap();
        reopened.recover_local_submissions().unwrap();
        assert_eq!(
            reopened.edit_barrier(id).unwrap(),
            Some(EditBarrier::RecoveryRequired)
        );
        assert!(reopened
            .latest_unconfirmed_submission(id)
            .unwrap()
            .is_none());
        assert!(reopened
            .reserve_local_submission(id, submission("peer", true))
            .is_err());
        assert_eq!(reopened.load(id).unwrap().rows, snapshot.rows);
    }

    #[test]
    fn failed_local_commit_leaves_the_exact_dispatched_receipt_and_history_intact() {
        let (dir, store, id, snapshot) = fixture();
        let mut edit = store.admit_edit_rewind(id, snapshot.clone()).unwrap();
        store.dispatch_edit_rewind(&edit).unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER refuse_fork BEFORE UPDATE OF provider_session_id ON sessions
             WHEN NEW.provider_session_id='fork'
             BEGIN SELECT RAISE(ABORT,'fixture storage error'); END;",
            )
            .unwrap();
        assert!(store.finish_edit_last_turn(&mut edit, "fork").is_err());
        let peer = Store::open_vorkspace(&dir.path().join("native.db")).unwrap();
        let receipts = peer.local_submissions(id).unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].phase, "dispatching");
        assert_eq!(peer.load(id).unwrap().rows, snapshot.rows);
        drop(edit);
        peer.recover_local_submissions().unwrap();
        assert_eq!(
            peer.edit_barrier(id).unwrap(),
            Some(EditBarrier::RecoveryRequired)
        );
    }
}
