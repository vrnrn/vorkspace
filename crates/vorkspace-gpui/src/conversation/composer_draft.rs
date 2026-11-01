//! The desktop draft row remains authoritative across UI versions. Pending
//! input belongs to the view; a background flush compares its durable revision.
use super::*;
use serde_json::{Value, json};
use std::sync::Mutex;

pub(super) struct State {
    pub(super) writer: Arc<Mutex<Writer>>,
    timer: Option<gpui::Task<()>>,
    restoring: bool,
    touched: bool,
    hydrating: bool,
    pub(super) error: Option<String>,
    resolving: bool,
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
                discarded: false,
            })),
            timer: None,
            restoring: false,
            touched: false,
            hydrating: true,
            error: None,
            resolving: false,
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
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.discarded = true;
        writer.pending = None;
    }
    pub(super) fn generation(&self) -> u64 {
        self.writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .generation
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
    let result = locked(&state.store)?.save_composer_draft(state.id, state.revision, payload);
    let revision = match result {
        Ok(revision) => revision,
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
    let (saved, recoveries) = {
        let store = locked(&store)?;
        (store.composer_draft(id)?, store.composer_recoveries(id)?)
    };
    let mut state = writer.lock().unwrap_or_else(|e| e.into_inner());
    // A quit flush may have hydrated and saved while this read was in flight.
    if state.loaded {
        return Ok(current_record(&state));
    }
    if let Some(saved) = &saved {
        state.revision = saved.revision;
        state.base = saved.payload.clone();
        state.deleted = saved.deleted;
        if let Some(pending) = state.pending.take() {
            let mut merged = saved.payload.as_object().cloned().unwrap_or_default();
            if let Some(fields) = pending.as_object() {
                merged.extend(fields.clone());
            }
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
            let recovered = state.recoveries.last()?;
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
        let disabled = self.composer_draft.resolving;
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
                            crate::settings::widgets::secondary_button(
                                "composer-recover",
                                "Restore recovered input",
                                true,
                                disabled,
                            )
                            .child("Restore recovered input")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.resolve_composer_recovery(true, cx)
                            })),
                        )
                        .child(
                            crate::settings::widgets::secondary_button(
                                "composer-use-saved",
                                "Use saved input",
                                false,
                                disabled,
                            )
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
            self.settings.plan = mode == Some("plan");
            self.settings.operator = mode == Some("operator");
            self.settings.orchestrator = mode == Some("orchestrator");
            self.bot_ui.queue = payload["botSubmission"]["queue"].as_bool().unwrap_or(false);
            self.save_settings(cx);
        }
    }
    fn resolve_composer_recovery(&mut self, recovered: bool, cx: &mut Context<Self>) {
        if self.composer_draft.resolving {
            return;
        }
        if self.composer_draft.error.is_some() {
            self.capture_composer(cx);
            let _ = self.composer_draft.flush();
        }
        let writer = self.composer_draft.writer.clone();
        let (choice, generation) = {
            let state = writer.lock().unwrap_or_else(|e| e.into_inner());
            (state.recoveries.last().cloned(), state.generation)
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
            let (draft,remaining)={
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
                ((payload,revision),store.composer_recoveries(state.id)?)
            };
            state.base=draft.0.clone();state.revision=draft.1;state.loaded=true;
            state.deleted=state.base["text"].as_str()==Some("") && state.base["attachments"].as_array().is_some_and(Vec::is_empty);
            if state.generation==generation{state.pending=None;}
            state.recoveries=remaining;
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
            let _ = this.update(cx, |this, cx| match result {
                Ok(saved) => {
                    this.composer_draft.hydrating = false;
                    cx.notify();
                    if !this.composer_draft.touched
                        && let Some(saved) = saved
                        && !saved.deleted
                    {
                        let published = match this.composer_is_published_bot_draft(&saved) {
                            Ok(published) => published,
                            Err(error) => {
                                this.status = format!("Could not verify saved draft: {error}. Your input was preserved.");
                                this.composer_draft.error = Some(this.status.clone());
                                cx.notify();
                                return;
                            }
                        };
                        if published {
                            // The saved card/outbox owns exactly this old input.
                            // A newer revision, even with identical text, stays put.
                            this.persist_composer(cx);
                            return;
                        }
                        this.composer_draft.restoring = true;
                        let payload = &saved.payload;
                        this.attachments = payload["attachments"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|file| file["path"].as_str().map(std::path::PathBuf::from))
                            .collect();
                        this.input.update(cx, |input, cx| {
                            input.set_text(payload["text"].as_str().unwrap_or_default().into(), cx);
                            let start = payload["selectionStart"].as_u64().unwrap_or(0) as usize;
                            let end =
                                payload["selectionEnd"].as_u64().unwrap_or(start as u64) as usize;
                            input.select_utf16(start..end, cx);
                        });
                        this.completion.update(cx, |completion, cx| {
                            completion.restore_mcp_tags(&payload["mcpTags"], cx)
                        });
                        this.composer_draft.restoring = false;
                        cx.notify();
                    } else if this.composer_draft.touched {
                        this.persist_composer(cx);
                    }
                }
                Err(error) => {
                    this.status = format!("Could not restore composer: {error}");
                    this.composer_draft.error = Some(this.status.clone());
                    cx.notify();
                }
            });
        })
        .detach();
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
        if current["text"].as_str() == Some(input.content.as_ref())
            && current["selectionStart"].as_u64() == Some(selection.start as u64)
            && current["selectionEnd"].as_u64() == Some(selection.end as u64)
            && same_files
            && same_tags
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
        state.pending = Some(payload);
        state.generation = state.generation.wrapping_add(1);
        self.composer_draft.touched = true;
        true
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
                Ok(()) => this.composer_draft.error = None,
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
