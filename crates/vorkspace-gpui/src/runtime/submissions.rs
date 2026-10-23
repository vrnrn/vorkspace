//! Local intent is recorded before provider input. A timeout or cancellation
//! cannot prove that an external effect did not happen, and never permits replay.
use super::*;
use std::collections::HashMap;
use vorkspace_core::store::SubmissionInput;

pub(super) struct Submissions {
    store: Database,
    owner: Option<vorkspace_core::store::SubmissionOwner>,
    session: i64,
    initial: Option<String>,
    dispatched: bool,
    follow_ups: HashMap<String, String>,
}
impl Submissions {
    pub(super) fn new(store: Database, session: i64) -> Self {
        let owner = locked(&store)
            .ok()
            .map(|store| store.local_submission_owner());
        Self {
            store,
            owner,
            session,
            initial: None,
            dispatched: false,
            follow_ups: HashMap::new(),
        }
    }
    fn reserve(
        &self,
        user_turn: &str,
        text: &str,
        prepared: Option<&str>,
        attachments: &[std::path::PathBuf],
        settings: Option<&Settings>,
    ) -> Result<String, String> {
        let attempt = uuid();
        let metadata: Vec<_> = attachments.iter().map(|path| serde_json::json!({
            "id":path.to_string_lossy(), "name":path.file_name().unwrap_or_default().to_string_lossy(),
            "kind":"file", "mimeType":"application/octet-stream", "size":std::fs::metadata(path).map(|m|m.len()).unwrap_or(0), "path":path.to_string_lossy()
        })).collect();
        let reservation = locked(&self.store)?.reserve_local_submission(self.session, SubmissionInput {
            attempt_id: attempt.clone(), user_turn_id: user_turn.into(), session_id: String::new(),
            payload_json: serde_json::json!({"text":text,"prepared":prepared,"attachments":attachments,"settings":settings}).to_string(),
            user_turn: Some(serde_json::json!({"id":user_turn,"role":"user","text":text,"attachments":metadata,"localSubmissionOwned":true}))
        })?;
        if !reservation.created {
            return Err("This local attempt was already accepted and was not replayed".into());
        }
        Ok(attempt)
    }
    pub(super) fn reserve_initial(
        &mut self,
        text: &str,
        settings: &Settings,
    ) -> Result<(), String> {
        let user_turn = settings.bot_message_id.clone().unwrap_or_else(uuid);
        self.initial = Some(self.reserve(
            &user_turn,
            text,
            settings.prepared_prompt.as_deref(),
            &settings.attachments,
            Some(settings),
        )?);
        Ok(())
    }
    pub(super) fn dispatch_initial(&mut self) -> Result<(), String> {
        let attempt = self.initial.as_ref().ok_or("Local send was not reserved")?;
        if !locked(&self.store)?
            .dispatch_local_submission(attempt)?
            .claimed
        {
            return Err("Local send is already claimed; it was not replayed".into());
        }
        self.dispatched = true;
        Ok(())
    }
    pub(super) fn dispatch_follow_up(
        &mut self,
        message: &vorkspace_core::follow_up::Message,
    ) -> Result<(), String> {
        let attempt = self.reserve(
            &message.id,
            &message.text,
            message.prepared_prompt.as_deref(),
            &message.attachments,
            None,
        )?;
        self.follow_ups.insert(message.id.clone(), attempt.clone());
        if !locked(&self.store)?
            .dispatch_local_submission(&attempt)?
            .claimed
        {
            return Err("This follow-up was already claimed; it was not replayed".into());
        }
        Ok(())
    }
    pub(super) fn accept_follow_up(&mut self, id: &str) -> Result<(), String> {
        if let Some(attempt) = self.follow_ups.get(id) {
            locked(&self.store)?.settle_local_submission(attempt, "completed", None)?;
            self.follow_ups.remove(id);
        }
        Ok(())
    }
    pub(super) fn fail_follow_up(&mut self, id: &str, error: &str) -> Result<(), String> {
        if let Some(attempt) = self.follow_ups.get(id) {
            locked(&self.store)?.settle_local_submission(
                attempt,
                "uncertain",
                Some(format!(
                    "{error}. The provider may have received this message; it was not replayed."
                )),
            )?;
            self.follow_ups.remove(id);
        }
        Ok(())
    }
    pub(super) fn finish(&mut self, result: &Result<(), String>) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Some(attempt) = self.initial.as_ref() {
            let outcome = if result.is_ok() {
                "completed"
            } else if self.dispatched {
                "uncertain"
            } else {
                "failed"
            };
            let error=result.as_ref().err().map(|error|if self.dispatched { format!("{error}. Provider completion could not be confirmed; this send was not replayed.") } else { error.clone() });
            match locked(&self.store)
                .and_then(|store| store.settle_local_submission(attempt, outcome, error))
            {
                Ok(_) => self.initial = None,
                Err(error) => errors.push(error),
            }
        }
        let mut settled = Vec::new();
        for (id, attempt) in &self.follow_ups {
            match locked(&self.store).and_then(|store| {
                store.settle_local_submission(
                    attempt,
                    "uncertain",
                    Some(format!(
                        "Follow-up {id} ended without an acknowledgement; it was not replayed."
                    )),
                )
            }) {
                Ok(_) => settled.push(id.clone()),
                Err(error) => errors.push(error),
            }
        }
        for id in settled {
            self.follow_ups.remove(&id);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(" · "))
        }
    }
}
impl Drop for Submissions {
    fn drop(&mut self) {
        // A finished worker cannot retain a live-process ownership claim after
        // a transient storage failure. The next normal recovery pass retries.
        if let Some(owner) = &self.owner {
            if let Some(attempt) = &self.initial {
                owner.release(attempt);
            }
            for attempt in self.follow_ups.values() {
                owner.release(attempt);
            }
        }
    }
}
fn uuid() -> String {
    // GPUI already depends on the core ID generator; no global/user identity.
    vorkspace_core::bot::new_id()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preflight_failure_is_retryable_but_dispatched_unknown_effects_never_replay() {
        let db = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
        let session = db.create("codex", "/project").unwrap().id;
        let store = Arc::new(Mutex::new(db));
        let settings = Settings {
            bot_message_id: Some("queued-message".into()),
            ..Default::default()
        };
        let mut failed = Submissions::new(store.clone(), session);
        failed.reserve_initial("hello", &settings).unwrap();
        failed.finish(&Err("CLI missing".into())).unwrap();
        assert_eq!(
            locked(&store).unwrap().local_submissions(session).unwrap()[0]
                .outcome
                .as_deref(),
            Some("failed")
        );
        let mut interrupted = Submissions::new(store.clone(), session);
        interrupted.reserve_initial("hello", &settings).unwrap();
        interrupted.dispatch_initial().unwrap();
        interrupted.finish(&Err("Disconnected".into())).unwrap();
        let recovery = locked(&store)
            .unwrap()
            .latest_unconfirmed_submission(session)
            .unwrap()
            .unwrap();
        assert_eq!(recovery.user_turn.unwrap()["text"], "hello");
        let mut retry = Submissions::new(store.clone(), session);
        assert!(
            retry
                .reserve_initial("hello", &settings)
                .unwrap_err()
                .contains("not replayed")
        );
        locked(&store)
            .unwrap()
            .acknowledge_local_submission(session, &recovery.attempt_id)
            .unwrap();
        assert!(
            locked(&store)
                .unwrap()
                .latest_unconfirmed_submission(session)
                .unwrap()
                .is_none()
        );
        assert!(retry.reserve_initial("hello", &settings).is_err());
    }
    #[test]
    fn failed_steer_keeps_its_recovery_metadata_and_blocks_the_stable_queue_identity() {
        let db = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
        let session = db.create("codex", "/project").unwrap().id;
        let store = Arc::new(Mutex::new(db));
        let message = vorkspace_core::follow_up::Message {
            id: "queued-message".into(),
            text: "turn left".into(),
            attachments: vec!["/private/cached.png".into()],
            ..Default::default()
        };
        let mut submissions = Submissions::new(store.clone(), session);
        submissions.dispatch_follow_up(&message).unwrap();
        submissions
            .fail_follow_up(&message.id, "Acknowledgement timed out")
            .unwrap();
        let recovery = locked(&store)
            .unwrap()
            .latest_unconfirmed_submission(session)
            .unwrap()
            .unwrap();
        assert_eq!(
            recovery.user_turn.unwrap()["attachments"][0]["path"],
            "/private/cached.png"
        );
        assert!(submissions.dispatch_follow_up(&message).is_err());
    }
}

#[cfg(test)]
mod unwind_tests {
    use super::*;
    #[test]
    fn poisoned_store_mutex_does_not_retain_a_finished_workers_claim() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sessions.db");
        let db = Store::open_vorkspace(&path).unwrap();
        let session = db.create("codex", "/project").unwrap().id;
        let db = Arc::new(Mutex::new(db));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut submissions = Submissions::new(db.clone(), session);
            submissions
                .reserve_initial("Preserved while unwinding", &Settings::default())
                .unwrap();
            submissions.dispatch_initial().unwrap();
            let _guard = locked(&db).unwrap();
            panic!("Controlled worker panic while owning the store lock");
        }));
        assert!(result.is_err());
        assert!(db.is_poisoned());
        let reopened = Store::open_vorkspace(&path).unwrap();
        reopened.recover_local_submissions().unwrap();
        let receipt = reopened
            .latest_unconfirmed_submission(session)
            .unwrap()
            .unwrap();
        assert_eq!(
            receipt.user_turn.unwrap()["text"],
            "Preserved while unwinding"
        );
    }
}
