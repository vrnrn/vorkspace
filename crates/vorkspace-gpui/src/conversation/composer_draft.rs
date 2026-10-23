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
    pub(super) fn ready_for_replacement(&self) -> bool {
        !self.hydrating && self.error.is_none() && !self.resolving
    }
}
pub(super) fn flush(writer: &Arc<Mutex<Writer>>) -> Result<(), String> {
    let (pending, loaded) = {
        let state = writer.lock().unwrap_or_else(|e| e.into_inner());
        (state.pending.is_some(), state.loaded)
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
impl Conversation {
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
                let (payload,revision)=if recovered {
                    let revision=store.save_composer_draft(state.id,latest.as_ref().map_or(0,|d|d.revision),&choice.payload)?;
                    (choice.payload,revision)
                }else{latest.map(|d|(d.payload,d.revision)).unwrap_or_else(||(json!({"cwd":cwd,"text":"","selectionStart":0,"selectionEnd":0,"attachments":[],"mcpTags":[]}),0))};
                store.clear_composer_recovery(state.id,&choice.owner)?;
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
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use vorkspace_core::store::Store;
    fn saved() -> Value {
        json!({"cwd":"/project","text":"Keep 🙂 @mcp/docs","selectionStart":5,"selectionEnd":7,
            "attachments":[{"id":"photo-id","path":"/private/photo.png","name":"photo.png","kind":"image","mimeType":"image/png","size":42,"opaque":"retain"}],
            "mcpTags":[{"token":"@mcp/docs","opaque":"keep","server":{"provider":"codex","name":"docs","scope":"user","configPath":"/private/config.toml","transport":"stdio","enabled":true}}],"future":"preserve"})
    }
    #[gpui::test]
    fn desktop_draft_restores_text_selection_files_and_mcp_then_saves_to_same_row(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vorkspace.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let id = session.id;
        store.save_composer_draft(id, 0, &saved()).unwrap();
        let db = Arc::new(Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session.clone(), db.clone(), cx));
        cx.run_until_parked();
        assert_eq!(
            locked(&db)
                .unwrap()
                .composer_draft(id)
                .unwrap()
                .unwrap()
                .revision,
            1
        );
        chat.update(cx, |chat, cx| {
            assert_eq!(chat.input.read(cx).content, "Keep 🙂 @mcp/docs");
            assert_eq!(chat.input.read(cx).selection_utf16(), 5..7);
            assert_eq!(
                chat.attachments,
                vec![std::path::PathBuf::from("/private/photo.png")]
            );
            assert!(
                chat.completion
                    .read(cx)
                    .prepare_prompt("Use @mcp/docs")
                    .starts_with("MCP context:")
            );
            chat.input.update(cx, |input, cx| {
                input.set_text("Edited 🙂 @mcp/docs".into(), cx)
            });
            chat.capture_composer(cx);
            chat.composer_draft.flush().unwrap();
        });
        drop(chat);
        drop(db);
        let store = Store::open_vorkspace(&path).unwrap();
        let record = store.composer_draft(id).unwrap().unwrap();
        assert_eq!(record.revision, 2);
        assert_eq!(record.payload["text"], "Edited 🙂 @mcp/docs");
        assert_eq!(record.payload["attachments"], saved()["attachments"]);
        assert_eq!(record.payload["mcpTags"], saved()["mcpTags"]);
        assert_eq!(record.payload["future"], "preserve");
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        cx.run_until_parked();
        chat.read_with(cx, |chat, cx| {
            assert_eq!(chat.input.read(cx).content, "Edited 🙂 @mcp/docs")
        });
    }
    #[gpui::test]
    fn typing_during_draft_load_wins_and_clear_keeps_a_revision_tombstone(cx: &mut TestAppContext) {
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let id = session.id;
        store.save_composer_draft(id, 0, &saved()).unwrap();
        let db = Arc::new(Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session.clone(), db.clone(), cx));
        chat.update(cx, |chat, cx| {
            chat.input
                .update(cx, |input, cx| input.set_text("New intent".into(), cx));
            chat.persist_composer(cx);
        });
        cx.run_until_parked();
        chat.update(cx, |chat, cx| {
            assert_eq!(chat.input.read(cx).content, "New intent");
            assert!(chat.attachments.is_empty());
            chat.capture_composer(cx);
            chat.composer_draft.flush().unwrap();
            chat.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            chat.capture_composer(cx);
            chat.composer_draft.flush().unwrap();
        });
        let saved = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
        assert!(saved.deleted);
        assert_eq!(saved.revision, 3);
        let reopened = cx.new(|cx| Conversation::new(session, db, cx));
        cx.run_until_parked();
        reopened.read_with(cx, |chat, cx| {
            assert!(chat.input.read(cx).content.is_empty())
        });
    }
    #[gpui::test]
    fn quit_flush_before_load_completion_retains_the_latest_input(cx: &mut TestAppContext) {
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let id = session.id;
        store.save_composer_draft(id, 0, &saved()).unwrap();
        let db = Arc::new(Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
        chat.update(cx, |chat, cx| {
            chat.input.update(cx, |input, cx| {
                input.set_text("New text before restore completes".into(), cx)
            });
            chat.capture_composer(cx);
            chat.composer_draft.flush().unwrap();
        });
        cx.run_until_parked();
        let record = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
        assert_eq!(record.payload["text"], "New text before restore completes");
        assert_eq!(record.payload["future"], "preserve");
        chat.read_with(cx, |chat, cx| {
            assert_eq!(
                chat.input.read(cx).content,
                "New text before restore completes"
            )
        });
    }
    #[gpui::test]
    fn same_mcp_token_saves_changed_server_identity_once_and_keeps_opaque_fields(
        cx: &mut TestAppContext,
    ) {
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let id = session.id;
        store.save_composer_draft(id, 0, &saved()).unwrap();
        let db = Arc::new(Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
        cx.run_until_parked();
        chat.update(cx, |chat, cx| {
            let mut tags = saved()["mcpTags"].clone();
            tags[0]["server"]["provider"] = json!("claude");
            tags[0]["server"]["configPath"] = json!("/other/config.json");
            chat.completion
                .update(cx, |completion, cx| completion.restore_mcp_tags(&tags, cx));
            assert!(chat.capture_composer(cx));
            chat.composer_draft.flush().unwrap();
            assert!(!chat.capture_composer(cx));
        });
        let draft = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
        assert_eq!(draft.revision, 2);
        assert_eq!(draft.payload["mcpTags"][0]["server"]["provider"], "claude");
        assert_eq!(
            draft.payload["mcpTags"][0]["server"]["configPath"],
            "/other/config.json"
        );
        assert_eq!(draft.payload["mcpTags"][0]["opaque"], "keep");
    }
    #[gpui::test]
    fn conflicting_input_survives_close_reopen_and_explicit_restore_or_reload(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let id = session.id;
        store.save_composer_draft(id, 0, &saved()).unwrap();
        let db = Arc::new(Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session.clone(), db.clone(), cx));
        cx.run_until_parked();
        let other = Store::open_vorkspace(&path).unwrap();
        let mut external = saved();
        external["text"] = json!("Other window input");
        other.save_composer_draft(id, 1, &external).unwrap();
        chat.update(cx, |chat, cx| {
            chat.input.update(cx, |input, cx| {
                input.set_text("My pending input".into(), cx)
            });
            chat.capture_composer(cx);
            assert!(chat.composer_draft.flush().is_err());
            assert!(chat.composer_recovery_banner(cx).is_some());
        });
        drop(chat);
        cx.run_until_parked();
        assert_eq!(
            other.composer_draft(id).unwrap().unwrap().payload["text"],
            "Other window input"
        );
        let reopened_store = Store::open_vorkspace(&path).unwrap();
        assert_eq!(reopened_store.composer_recoveries(id).unwrap().len(), 1);
        let reopened =
            cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(reopened_store)), cx));
        cx.run_until_parked();
        reopened.update(cx, |chat, cx| {
            assert_eq!(chat.input.read(cx).content, "Other window input");
            chat.resolve_composer_recovery(true, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            other.composer_draft(id).unwrap().unwrap().payload["text"],
            "My pending input"
        );
        assert!(other.composer_recoveries(id).unwrap().is_empty());
        external["text"] = json!("Newest other input");
        other.save_composer_draft(id, 3, &external).unwrap();
        reopened.update(cx, |chat, cx| {
            chat.input.update(cx, |input, cx| {
                input.set_text("My second pending input".into(), cx)
            });
            chat.capture_composer(cx);
            assert!(chat.composer_draft.flush().is_err());
            chat.resolve_composer_recovery(false, cx);
        });
        cx.run_until_parked();
        reopened.read_with(cx, |chat, cx| {
            assert_eq!(chat.input.read(cx).content, "Newest other input")
        });
        assert_eq!(other.composer_draft(id).unwrap().unwrap().revision, 4);
        assert!(other.composer_recoveries(id).unwrap().is_empty());
    }
    #[gpui::test]
    fn selection_changes_during_recovery_publication_preserve_the_newer_composer(
        cx: &mut TestAppContext,
    ) {
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let id = session.id;
        store.save_composer_draft(id, 0, &saved()).unwrap();
        let mut recovery = saved();
        recovery["text"] = json!("Recovered different text");
        store
            .save_composer_recovery(
                id,
                vorkspace_core::store::ComposerRecovery {
                    owner: "previous-window".into(),
                    revision: 0,
                    payload: recovery,
                },
            )
            .unwrap();
        let db = Arc::new(Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session, db.clone(), cx));
        cx.run_until_parked();
        chat.update(cx, |chat, cx| {
            chat.resolve_composer_recovery(true, cx);
            chat.input
                .update(cx, |input, cx| input.select_utf16(0..4, cx));
        });
        cx.run_until_parked();
        chat.update(cx, |chat, cx| {
            assert_eq!(chat.input.read(cx).content, "Keep 🙂 @mcp/docs");
            assert_eq!(chat.input.read(cx).selection_utf16(), 0..4);
            chat.capture_composer(cx);
            chat.composer_draft.flush().unwrap();
        });
        let record = locked(&db).unwrap().composer_draft(id).unwrap().unwrap();
        assert_eq!(record.payload["selectionStart"], 0);
        assert_eq!(record.payload["selectionEnd"], 4);
    }
}
