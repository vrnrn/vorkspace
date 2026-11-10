//! The desktop draft row remains authoritative across UI versions. Pending
//! input belongs to the view; a background flush compares its durable revision.
use super::*;
use serde_json::{Value, json};
use std::sync::Mutex;
use vorkspace_core::store::ComposerOrigin;

pub(super) struct State {
    pub(super) writer: Arc<Mutex<Writer>>,
    timer: Option<gpui::Task<()>>,
    restoring: bool,
    touched: bool,
    hydrating: bool,
    pub(super) error: Option<String>,
    resolving: bool,
    submissions: Vec<PendingSubmission>,
    send_after_restore: Option<u64>,
}

struct PendingSubmission {
    origin: ComposerOrigin,
    generation: u64,
    lane: ComposerLane,
    recovery_owner: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ComposerLane {
    Turn,
    Shelf,
}
pub(super) struct Writer {
    store: Database,
    id: i64,
    loaded: bool,
    revision: u64,
    base: Value,
    pending: Option<Value>,
    deleted: bool,
    owner: String,
    recoveries: Vec<vorkspace_core::store::ComposerRecovery>,
    generation: u64,
    displaced_offer: Option<ComposerOrigin>,
    discarded: bool,
}
impl State {
    pub(super) fn new(store: Database, id: i64) -> Self {
        Self {
            writer: Arc::new(Mutex::new(Writer {
                store,
                id,
                loaded: false,
                revision: 0,
                base: Value::Null,
                pending: None,
                deleted: false,
                owner: vorkspace_core::bot::new_id(),
                recoveries: Vec::new(),
                generation: 0,
                displaced_offer: None,
                discarded: false,
            })),
            timer: None,
            restoring: false,
            touched: false,
            hydrating: true,
            error: None,
            resolving: false,
            submissions: Vec::new(),
            send_after_restore: None,
        }
    }
    pub(super) fn flush(&self) -> Result<(), String> {
        flush(&self.writer)
    }
    /// A failed fresh-session installation has no accepted composer owner.
    /// Serialize with any running flush before its session is removed, and
    /// prevent delayed hydration or Drop from recreating an orphan draft.
    pub(super) fn discard(&mut self) {
        self.timer = None;
        self.send_after_restore = None;
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.discarded = true;
        writer.pending = None;
        writer.displaced_offer = None;
    }
    pub(super) fn generation(&self) -> u64 {
        self.writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .generation
    }
    pub(super) fn submitting(&self) -> bool {
        !self.submissions.is_empty()
    }

    pub(super) fn lane_pending(&self, lane: ComposerLane) -> bool {
        self.submissions.iter().any(|offer| offer.lane == lane)
    }

    fn take_submission(&mut self, origin: &ComposerOrigin) -> Option<PendingSubmission> {
        let at = self
            .submissions
            .iter()
            .position(|offer| &offer.origin == origin)?;
        Some(self.submissions.swap_remove(at))
    }

    fn visible_recovery<'a>(
        &self,
        writer: &'a Writer,
    ) -> Option<&'a vorkspace_core::store::ComposerRecovery> {
        writer.recoveries.iter().rev().find(|record| {
            self.submissions
                .iter()
                .all(|offer| offer.recovery_owner != record.owner)
        })
    }
    pub(super) fn waiting_to_submit(&self) -> bool {
        self.send_after_restore.is_some()
    }
    pub(super) fn cancel_waiting_send(&mut self) -> bool {
        self.send_after_restore.take().is_some()
    }
    pub(super) fn forget_recovery(&self, owner: &str) {
        self.writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .recoveries
            .retain(|record| record.owner != owner);
    }
    pub(super) fn ready_for_replacement(&self) -> bool {
        !self.hydrating
            && self.error.is_none()
            && !self.resolving
            && !self
                .writer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .discarded
    }
}
pub(super) fn flush(writer: &Arc<Mutex<Writer>>) -> Result<(), String> {
    let (pending, loaded) = {
        let state = writer.lock().unwrap_or_else(|e| e.into_inner());
        (state.pending.is_some() && !state.discarded, state.loaded)
    };
    if !pending {
        return Ok(());
    }
    if !loaded {
        hydrate(writer)?;
    }
    let mut state = writer.lock().unwrap_or_else(|e| e.into_inner());
    let Some(payload) = state.pending.as_ref() else {
        return Ok(());
    };
    let result = {
        let store = locked(&state.store)?;
        if let Some(origin) = &state.displaced_offer {
            store.save_composer_after_local_offer(origin, payload)
        } else {
            store
                .save_composer_draft(state.id, state.revision, payload)
                .map(|revision| (revision, None))
        }
    };
    let (revision, recovery) = match result {
        Ok(saved) => saved,
        Err(error) => {
            let recovery = vorkspace_core::store::ComposerRecovery {
                owner: state.owner.clone(),
                revision: state.revision,
                payload: payload.clone(),
            };
            locked(&state.store)?.save_composer_recovery(state.id, recovery.clone())?;
            state.recoveries.retain(|old| old.owner != recovery.owner);
            state.recoveries.push(recovery);
            return Err(error);
        }
    };
    if let Some(recovery) = recovery {
        state.recoveries.retain(|old| old.owner != recovery.owner);
        state.recoveries.push(recovery);
    }
    state.displaced_offer = None;
    state.revision = revision;
    state.base = state.pending.take().unwrap();
    state.deleted = state.base["text"].as_str() == Some("")
        && state.base["attachments"]
            .as_array()
            .is_some_and(Vec::is_empty);
    Ok(())
}
fn hydrate(
    writer: &Arc<Mutex<Writer>>,
) -> Result<Option<vorkspace_core::store::ComposerDraft>, String> {
    let (store, id) = {
        let state = writer.lock().unwrap_or_else(|e| e.into_inner());
        if state.loaded {
            return Ok(current_record(&state));
        }
        (state.store.clone(), state.id)
    };
    let (saved, mut recoveries) = {
        let store = locked(&store)?;
        (store.composer_draft(id)?, store.composer_recoveries(id)?)
    };
    let mut state = writer.lock().unwrap_or_else(|e| e.into_inner());
    // A quit flush may have hydrated and saved while this read was in flight.
    if state.loaded {
        return Ok(current_record(&state));
    }
    if let Some(saved) = &saved {
        if state.pending.as_ref().is_some_and(|pending| {
            !saved.deleted && !same_submission_input(&saved.payload, pending)
        }) {
            // An editor can accept typing before its first read arrives. Keep
            // the unseen saved version recoverable before publishing that edit.
            let recovery = vorkspace_core::store::ComposerRecovery {
                owner: format!("previous-{}", state.owner),
                revision: saved.revision,
                payload: saved.payload.clone(),
            };
            locked(&store)?.save_composer_recovery(id, recovery.clone())?;
            recoveries.push(recovery);
        }
        state.revision = saved.revision;
        state.base = saved.payload.clone();
        state.deleted = saved.deleted;
        if let Some(pending) = state.pending.take() {
            let mut merged = saved.payload.as_object().cloned().unwrap_or_default();
            if let Some(fields) = pending.as_object() {
                merged.extend(fields.clone());
            }
            merged.remove("botSubmission");
            state.pending = Some(Value::Object(merged));
        }
    }
    state.loaded = true;
    state.recoveries = recoveries;
    Ok(saved)
}
fn current_record(state: &Writer) -> Option<vorkspace_core::store::ComposerDraft> {
    (!state.base.is_null()).then(|| vorkspace_core::store::ComposerDraft {
        revision: state.revision,
        payload: state.base.clone(),
        deleted: state.deleted,
    })
}
fn same_submission_input(a: &Value, b: &Value) -> bool {
    a["text"]
        .as_str()
        .is_some_and(|text| b["text"].as_str() == Some(text))
        && a["cwd"] == b["cwd"]
        && a["attachments"]
            .as_array()
            .zip(b["attachments"].as_array())
            .is_some_and(|(a, b)| {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| {
                        a["path"]
                            .as_str()
                            .is_some_and(|path| b["path"].as_str() == Some(path))
                    })
            })
        && a["mcpTags"]
            .as_array()
            .zip(b["mcpTags"].as_array())
            .is_some_and(|(a, b)| {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| {
                        a["token"] == b["token"]
                            && ["provider", "name", "scope", "configPath", "transport"]
                                .into_iter()
                                .all(|key| a["server"][key] == b["server"][key])
                            && a["server"]["enabled"].as_bool().unwrap_or(true)
                                == b["server"]["enabled"].as_bool().unwrap_or(true)
                    })
            })
}
impl Conversation {
    pub(super) fn defer_send_until_composer_ready(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.composer_draft.hydrating {
            return false;
        }
        let input = self.input.read(cx);
        if !input.is_composing() && (input.has_message_text() || !self.attachments.is_empty()) {
            self.persist_composer(cx);
            self.composer_draft.send_after_restore = Some(self.composer_draft.generation());
            cx.notify();
        }
        true
    }

    pub(super) fn composer_submission_pending(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.composer_draft.submitting() {
            return false;
        }
        self.persist_composer(cx);
        self.composer_version_offered()
    }

    pub(super) fn composer_version_offered(&self) -> bool {
        let generation = self.composer_draft.generation();
        self.composer_draft
            .submissions
            .iter()
            .any(|offer| offer.generation == generation)
    }

    pub(super) fn prepare_composer_submission(
        &mut self,
        lane: ComposerLane,
        cx: &mut Context<Self>,
    ) -> Result<ComposerOrigin, String> {
        if !self.composer_draft.ready_for_replacement() {
            return Err("Wait for the current draft to finish loading or sending.".into());
        }
        self.capture_composer(cx);
        if self.composer_version_offered() || self.composer_draft.lane_pending(lane) {
            return Err(
                "Wait for the previous message to finish saving. Your input was preserved.".into(),
            );
        }
        self.composer_draft.flush()?;
        let (revision, generation) = {
            let writer = self
                .composer_draft
                .writer
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (writer.revision, writer.generation)
        };
        let origin = locked(&self.store)?.composer_origin(self.summary.id, revision)?;
        self.composer_draft.submissions.push(PendingSubmission {
            recovery_owner: origin.recovery_owner(),
            origin: origin.clone(),
            generation,
            lane,
        });
        Ok(origin)
    }

    pub(super) fn accept_composer_submission(
        &mut self,
        origin: &ComposerOrigin,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self
            .composer_draft
            .submissions
            .iter()
            .position(|offer| &offer.origin == origin)
        else {
            return;
        };
        self.persist_composer(cx);
        let offer = self.composer_draft.submissions.swap_remove(at);
        self.composer_draft.forget_recovery(&offer.recovery_owner);
        let cleared = self.composer_draft.generation() == offer.generation
            && !self.input.read(cx).is_composing();
        if cleared {
            let clear_mode = self.follow_ups.draft_selected || self.selected_turn_mode().is_some();
            self.attachments.clear();
            self.follow_ups.draft_selected = false;
            self.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            self.persist_composer(cx);
            if clear_mode {
                self.clear_submitted_mode(cx);
            }
        }
        cx.notify();
    }

    pub(super) fn finish_composer_submission(
        &mut self,
        origin: &ComposerOrigin,
        cx: &mut Context<Self>,
    ) {
        if self.composer_draft.take_submission(origin).is_some() {
            self.persist_composer(cx);
        }
    }

    pub(super) fn settle_composer_submission(
        &mut self,
        origin: &ComposerOrigin,
        cx: &mut Context<Self>,
    ) {
        if !self
            .composer_draft
            .submissions
            .iter()
            .any(|offer| &offer.origin == origin)
        {
            return;
        }
        let committed =
            locked(&self.store).and_then(|store| store.composer_transfer_committed(origin));
        match committed {
            Ok(true) => {
                self.accept_composer_submission(origin, cx);
            }
            Ok(false) => self.finish_composer_submission(origin, cx),
            // Keep the offered version fenced until storage or a newer edit
            // supplies ownership. An uncertain read must not recreate it.
            Err(_) => {}
        }
    }

    /// A direct Bot send may have reached the outbox before its UI
    /// acknowledgement. Reuse its durable identity after reopening, so that
    /// neither a queued message nor a dispatch claim can be submitted twice.
    pub(super) fn pending_bot_submission(
        &self,
        text: &str,
        attachments: &[std::path::PathBuf],
    ) -> Result<Option<vorkspace_core::store::ComposerRecovery>, String> {
        let store = locked(&self.store)?;
        let saved = store.composer_draft(self.summary.id)?;
        let records = store.composer_recoveries(self.summary.id)?;
        let mut matches = records.into_iter().filter(|record| {
            record.payload["botSubmission"].is_object()
                && saved
                    .as_ref()
                    .is_some_and(|draft| same_submission_input(&record.payload, &draft.payload))
                && record.payload["text"].as_str() == Some(text)
                && record.payload["attachments"]
                    .as_array()
                    .is_some_and(|files| {
                        files.len() == attachments.len()
                            && files.iter().zip(attachments).all(|(file, path)| {
                                file["path"]
                                    .as_str()
                                    .is_some_and(|saved| Path::new(saved) == path)
                            })
                    })
        });
        let first = matches.next();
        if matches.next().is_some() {
            return Err(
                "Multiple recovered submissions match this input; review them before sending"
                    .into(),
            );
        }
        Ok(first)
    }
    /// Flush the input so the database can compare its revision when publishing the draft.
    pub(super) fn bot_draft_origin_revision(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        if !self.composer_draft.ready_for_replacement() {
            return Err("The composer is still restoring".into());
        }
        self.capture_composer(cx);
        self.composer_draft.flush()?;
        let revision = self
            .composer_draft
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .revision;
        Ok(revision)
    }

    pub(super) fn mark_bot_draft_discarded(&self, draft_id: &str) -> Result<(), String> {
        let store = locked(&self.store)?;
        let key = format!("bot-draft-origin:{}", self.summary.id);
        let Some(raw) = store.setting(&key)? else {
            return Ok(());
        };
        let mut marker: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if marker["id"].as_str() == Some(draft_id) || marker["nextId"].as_str() == Some(draft_id) {
            marker["discarded"] = json!(true);
            store.compare_swap_setting(&key, Some(&raw), &marker.to_string())?;
        }
        Ok(())
    }

    pub(super) fn mark_bot_draft_edit_origin(
        &self,
        old_id: &str,
        new_id: &str,
    ) -> Result<(), String> {
        let store = locked(&self.store)?;
        let key = format!("bot-draft-origin:{}", self.summary.id);
        let Some(raw) = store.setting(&key)? else {
            return Ok(());
        };
        let mut marker: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if marker["id"].as_str() == Some(old_id) || marker["nextId"].as_str() == Some(old_id) {
            marker["id"] = json!(old_id);
            marker["nextId"] = json!(new_id);
            store.compare_swap_setting(&key, Some(&raw), &marker.to_string())?;
        }
        Ok(())
    }

    fn composer_is_published_bot_draft(
        &self,
        saved: &vorkspace_core::store::ComposerDraft,
    ) -> Result<bool, String> {
        let store = locked(&self.store)?;
        let Some(raw) = store.setting(&format!("bot-draft-origin:{}", self.summary.id))? else {
            return Ok(false);
        };
        let marker: Value =
            serde_json::from_str(&raw).map_err(|e| format!("Cannot verify draft origin: {e}"))?;
        if marker["revision"].as_u64() != Some(saved.revision) {
            return Ok(false);
        }
        let id = marker["id"]
            .as_str()
            .ok_or("Cannot verify draft origin identity")?;
        let next_id = marker["nextId"].as_str();
        if marker["discarded"] == true {
            return Ok(true);
        }
        if store
            .follow_ups(self.summary.id)?
            .draft
            .is_some_and(|draft| draft.id == id || Some(draft.id.as_str()) == next_id)
        {
            return Ok(true);
        }
        for candidate in [Some(id), next_id].into_iter().flatten() {
            if store
                .setting(&format!("bot-delivered:{}:{candidate}", self.summary.id))?
                .is_some()
            {
                return Ok(true);
            }
        }
        Ok(vorkspace_core::bot::Roster::load(&store)?
            .bots
            .iter()
            .any(|bot| {
                bot.session_id == Some(self.summary.id)
                    && bot
                        .outbox
                        .iter()
                        .any(|message| message.id == id || Some(message.id.as_str()) == next_id)
            }))
    }
    pub(super) fn composer_recovery_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (preview, attachments) = {
            let state = self
                .composer_draft
                .writer
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let recovered = self.composer_draft.visible_recovery(&state)?;
            (
                recovered.payload["text"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(160)
                    .collect::<String>(),
                recovered.payload["attachments"]
                    .as_array()
                    .map_or(0, Vec::len),
            )
        };
        let disabled = self.composer_draft.resolving || self.composer_draft.submitting();
        Some(
            div()
                .px_3()
                .pt_3()
                .flex()
                .flex_col()
                .gap_2()
                .text_size(px(12.))
                .text_color(content(0.8))
                .child("Recovered input is available. Choose which draft to keep.")
                .child(format!("{preview} ({attachments} attachments)"))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            crate::controls::secondary_button(
                                "composer-recover",
                                "Restore recovered input",
                                true,
                                disabled,
                            )
                            .debug_selector(|| "composer-recover".into())
                            .child("Restore recovered input")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.resolve_composer_recovery(true, cx)
                            })),
                        )
                        .child(
                            crate::controls::secondary_button(
                                "composer-use-saved",
                                "Use saved input",
                                false,
                                disabled,
                            )
                            .debug_selector(|| "composer-use-saved".into())
                            .child("Use saved input")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.resolve_composer_recovery(false, cx)
                            })),
                        ),
                )
                .into_any_element(),
        )
    }
    fn install_composer_payload(&mut self, payload: &Value, cx: &mut Context<Self>) {
        self.composer_draft.restoring = true;
        self.attachments = payload["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|file| file["path"].as_str().map(std::path::PathBuf::from))
            .collect();
        self.input.update(cx, |input, cx| {
            input.set_text(payload["text"].as_str().unwrap_or_default().into(), cx);
            let start = payload["selectionStart"].as_u64().unwrap_or(0) as usize;
            let end = payload["selectionEnd"].as_u64().unwrap_or(start as u64) as usize;
            input.select_utf16(start..end, cx);
        });
        self.completion.update(cx, |completion, cx| {
            completion.restore_mcp_tags(&payload["mcpTags"], cx)
        });
        self.composer_draft.restoring = false;
        if self.settings.bot.is_some() && payload["botSubmission"].is_object() {
            let mode = payload["botSubmission"]["mode"].as_str();
            // Editor hydration can finish before the session's settings load.
            // Restoring a composer choice must not publish partial settings.
            turn_flow::apply_turn_mode(&mut self.settings, mode);
            self.bot_ui.queue = payload["botSubmission"]["queue"].as_bool().unwrap_or(false);
        }
    }
    fn resolve_composer_recovery(&mut self, recovered: bool, cx: &mut Context<Self>) {
        if self.composer_draft.resolving || self.composer_draft.submitting() {
            return;
        }
        if self.composer_draft.error.is_some() {
            self.capture_composer(cx);
            let _ = self.composer_draft.flush();
        }
        let writer = self.composer_draft.writer.clone();
        let (choice, generation) = {
            let state = writer.lock().unwrap_or_else(|e| e.into_inner());
            (
                self.composer_draft.visible_recovery(&state).cloned(),
                state.generation,
            )
        };
        let Some(choice) = choice else {
            return;
        };
        let current = self.input.read(cx).content.clone();
        let current_selection = self.input.read(cx).selection_utf16();
        let current_tags = self.completion.read(cx).draft_mcp_tags();
        let files = self.attachments.clone();
        let cwd = self.summary.cwd.clone();
        self.composer_draft.resolving = true;
        let work=cx.background_executor().spawn(async move{
            let mut state=writer.lock().unwrap_or_else(|e|e.into_inner());
            let draft={
                let store=locked(&state.store)?;
                let latest=store.composer_draft(state.id)?;
                let retain_submission = choice.payload["botSubmission"].is_object()
                    && (recovered || latest.as_ref().is_some_and(|draft| same_submission_input(&choice.payload, &draft.payload)));
                let (payload,revision)=if recovered {
                    let revision=store.save_composer_draft(state.id,latest.as_ref().map_or(0,|d|d.revision),&choice.payload)?;
                    (choice.payload,revision)
                }else{latest.map(|d|(d.payload,d.revision)).unwrap_or_else(||(json!({"cwd":cwd,"text":"","selectionStart":0,"selectionEnd":0,"attachments":[],"mcpTags":[]}),0))};
                // A recovered direct submission retains its original identity
                // until the outbox acknowledges it, including across a second
                // crash after restoring the composer.
                if !retain_submission {
                    store.clear_composer_recovery(state.id,&choice.owner)?;
                }
                (payload,revision)
            };
            state.base=draft.0.clone();state.revision=draft.1;state.loaded=true;
            state.deleted=state.base["text"].as_str()==Some("") && state.base["attachments"].as_array().is_some_and(Vec::is_empty);
            if state.generation==generation{state.pending=None;}
            // The durable submission receipt survives until acknowledgement,
            // but this view has already resolved its recovery choice. Reloading
            // receipts here would offer the same choice again and erase edits.
            state.recoveries.retain(|record| record.owner != choice.owner);
            Ok::<_,String>(draft.0)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.composer_draft.resolving = false;
                match result {
                    Ok(payload) => {
                        let current_generation = this
                            .composer_draft
                            .writer
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .generation;
                        if current_generation == generation
                            && this.input.read(cx).content == current
                            && this.attachments == files
                            && this.input.read(cx).selection_utf16() == current_selection
                            && this.completion.read(cx).draft_mcp_tags() == current_tags
                        {
                            this.install_composer_payload(&payload, cx);
                        } else {
                            this.persist_composer(cx);
                        }
                        this.composer_draft.error = None;
                        this.status = "Ready".into();
                    }
                    Err(error) => {
                        this.status = format!("Could not recover input: {error}");
                        this.composer_draft.error = Some(this.status.clone());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn load_composer(&mut self, cx: &mut Context<Self>) {
        let writer = self.composer_draft.writer.clone();
        let work = cx.background_executor().spawn(async move {
            let source = hydrate(&writer)?;
            if let Err(error) = flush(&writer)
                && writer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .recoveries
                    .is_empty()
            {
                return Err(error);
            }
            Ok::<_, String>(source)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.composer_draft.hydrating = false;
                this.apply_composer_load(result, cx);
                if this.composer_draft.waiting_to_submit() {
                    this.capture_composer(cx);
                }
                if let Some(generation) = this.composer_draft.send_after_restore.take() {
                    if this.composer_draft.error.is_none()
                        && this.composer_draft.generation() == generation
                        && !this.input.read(cx).is_composing()
                    {
                        this.send(cx);
                    }
                    if !this.external_submission_pending() {
                        cx.emit(SessionChanged::Settled);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_composer_load(
        &mut self,
        result: Result<Option<vorkspace_core::store::ComposerDraft>, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(saved) => {
                if !self.composer_draft.touched
                    && let Some(saved) = saved
                    && !saved.deleted
                {
                    let published = match self.composer_is_published_bot_draft(&saved) {
                        Ok(published) => published,
                        Err(error) => {
                            self.status = format!(
                                "Could not verify saved draft: {error}. Your input was preserved."
                            );
                            self.composer_draft.error = Some(self.status.clone());
                            return;
                        }
                    };
                    if published {
                        // The saved card/outbox owns exactly this old input.
                        // A newer revision, even with identical text, stays put.
                        self.persist_composer(cx);
                        return;
                    }
                    self.install_composer_payload(&saved.payload, cx);
                } else if self.composer_draft.touched {
                    self.persist_composer(cx);
                }
            }
            Err(error) => {
                self.status = format!("Could not restore composer: {error}");
                self.composer_draft.error = Some(self.status.clone());
            }
        }
    }
    pub(super) fn capture_composer(&mut self, cx: &mut Context<Self>) -> bool {
        if self.composer_draft.restoring {
            return false;
        }
        let input = self.input.read(cx);
        let selection = input.selection_utf16();
        let tags = self.completion.read(cx).draft_mcp_tags();
        if self.composer_draft.hydrating
            && !self.composer_draft.touched
            && input.content.is_empty()
            && self.attachments.is_empty()
        {
            return false;
        }
        self.composer_draft.touched |= !input.content.is_empty() || !self.attachments.is_empty();
        let mut state = self
            .composer_draft
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if state.discarded {
            return false;
        }
        let current = state.pending.as_ref().unwrap_or(&state.base);
        let same_files = current["attachments"].as_array().is_some_and(|files| {
            files.len() == self.attachments.len()
                && files
                    .iter()
                    .zip(&self.attachments)
                    .all(|(file, path)| file["path"].as_str() == path.to_str())
        });
        let same_tags = current["mcpTags"]
            .as_array()
            .zip(tags.as_array())
            .is_some_and(|(old, new)| {
                old.len() == new.len()
                    && old.iter().zip(new).all(|(a, b)| {
                        a["token"] == b["token"]
                            && ["provider", "name", "scope", "configPath", "transport"]
                                .into_iter()
                                .all(|key| a["server"][key] == b["server"][key])
                            && a["server"]["enabled"].as_bool().unwrap_or(true)
                                == b["server"]["enabled"].as_bool().unwrap_or(true)
                    })
            });
        let same_input =
            current["text"].as_str() == Some(input.content.as_ref()) && same_files && same_tags;
        let same_selection = current["selectionStart"].as_u64() == Some(selection.start as u64)
            && current["selectionEnd"].as_u64() == Some(selection.end as u64);
        let offer = self
            .composer_draft
            .submissions
            .iter()
            .find(|offer| offer.generation == state.generation);
        // Moving the caret while this version is offered must not publish
        // another unsent copy of the same input after its receipt commits.
        if same_input
            && (same_selection || offer.is_some() || self.composer_draft.waiting_to_submit())
        {
            return false;
        }
        let mut payload = state.base.as_object().cloned().unwrap_or_default();
        // Submission metadata belongs to the recovery record, not to future
        // composer edits after that submission has been acknowledged.
        payload.remove("botSubmission");
        let old_files = payload.get("attachments").and_then(Value::as_array);
        let files:Vec<_>=self.attachments.iter().map(|path|{
            old_files.into_iter().flatten().find(|file|file["path"].as_str()==path.to_str()).cloned().unwrap_or_else(||json!({
                "id":path,"path":path,"name":path.file_name().unwrap_or_default().to_string_lossy(),"kind":"file","mimeType":"application/octet-stream","size":0
            }))
        }).collect();
        payload.insert("cwd".into(), json!(self.summary.cwd));
        payload.insert("text".into(), json!(input.content));
        payload.insert("selectionStart".into(), json!(selection.start));
        payload.insert("selectionEnd".into(), json!(selection.end));
        payload.insert("attachments".into(), json!(files));
        // Retain opaque tag/server fields for a restored selection.
        let tags = tags
            .as_array()
            .into_iter()
            .flatten()
            .map(|tag| {
                let mut saved = state.base["mcpTags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|old| old["token"] == tag["token"])
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if let (Some(target), Some(source)) = (saved.as_object_mut(), tag.as_object()) {
                    let mut server = target
                        .get("server")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(fields) = tag["server"].as_object() {
                        server.extend(fields.clone());
                    }
                    target.extend(source.clone());
                    target.insert("server".into(), Value::Object(server));
                }
                saved
            })
            .collect::<Vec<_>>();
        payload.insert("mcpTags".into(), json!(tags));
        let payload = Value::Object(payload);
        if state.pending.as_ref().unwrap_or(&state.base) == &payload {
            return false;
        }
        if state.base.is_null()
            && input.content.is_empty()
            && self.attachments.is_empty()
            && !self.composer_draft.touched
        {
            return false;
        }
        let explicit_mode = input::Command::leading(&input.content).is_some_and(|command| {
            matches!(
                command.name(),
                "draft" | "plan" | "operator" | "orchestrator"
            )
        });
        let replaced_offer = offer.is_some() && !same_input;
        if let Some(offer) = offer.filter(|_| replaced_offer && !self.is_remote()) {
            state.displaced_offer = Some(offer.origin.clone());
        }
        state.pending = Some(payload);
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        self.composer_draft.touched = true;
        if replaced_offer
            && !explicit_mode
            && (self.follow_ups.draft_selected || self.selected_turn_mode().is_some())
        {
            self.follow_ups.draft_selected = false;
            self.clear_submitted_mode(cx);
        }
        true
    }
    pub(crate) fn append_review_comment(
        &mut self,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.user_read_only() {
            return Err("This chat is read-only.".into());
        }
        let input = self.input.read(cx);
        if input.is_composing() {
            return Err("Finish typing in the destination chat, then try again.".into());
        }
        if !self.composer_draft.ready_for_replacement() {
            return Err(
                "Open the destination chat and wait for its draft to load, then try again.".into(),
            );
        }
        let draft = input.content.as_ref();
        let separator = if draft.is_empty() || draft.ends_with("\n\n") {
            ""
        } else if draft.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        let text = format!("{draft}{separator}{}\n\n", text.trim());
        self.input.update(cx, |input, cx| input.set_text(text, cx));
        Ok(())
    }

    pub(super) fn persist_composer(&mut self, cx: &mut Context<Self>) {
        if !self.capture_composer(cx) {
            return;
        }
        let writer = self.composer_draft.writer.clone();
        self.composer_draft.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            let work = cx
                .background_executor()
                .spawn(async move { flush(&writer) });
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.composer_draft.error = None;
                    let state = this
                        .composer_draft
                        .writer
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if this.composer_draft.visible_recovery(&state).is_some() {
                        cx.notify();
                    }
                }
                Err(error) => {
                    this.status = format!("Could not save composer: {error}");
                    this.composer_draft.error = Some(this.status.clone());
                    cx.notify();
                }
            });
        }));
    }
}

#[cfg(test)]
mod tests;
