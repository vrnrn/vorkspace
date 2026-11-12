//! Scoped Notes reads and writes for Bot and Operator commands.
use super::*;
use vorkspace_core::notes::{NoteUpsert, numeric_id};

impl Shell {
    pub(in crate::shell) fn app_notes_action(
        &mut self,
        caller: i64,
        assigned: &[String],
        action: &str,
        input: &Value,
        full_list: bool,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let projects = if let Some(requested) = input.get("project").and_then(Value::as_str) {
            if !assigned.iter().any(|project| {
                vorkspace_core::files::path_key(project)
                    == vorkspace_core::files::path_key(requested)
            }) {
                return Err("Project is outside the caller's assignments".into());
            }
            vec![requested.to_owned()]
        } else {
            assigned.to_vec()
        };
        let write = action == "notes.write";
        if projects.is_empty()
            || !projects.iter().all(|project| {
                if write {
                    self.app_caller_allowed(caller, project, cx)
                } else {
                    self.app_caller_can_read(caller, project, cx)
                }
            })
        {
            return Err("The caller no longer owns this project's app capability".into());
        }
        let project_keys: HashSet<_> = projects
            .iter()
            .map(|project| vorkspace_core::files::path_key(project))
            .collect();
        let in_scope = |cwd: Option<&str>| {
            cwd.is_some_and(|cwd| project_keys.contains(&vorkspace_core::files::path_key(cwd)))
        };
        let store = locked(&self.store)?;
        let catalog = store.list_note_summaries()?;
        if action == "notes.list" {
            let offset = input
                .get("offset")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(usize::MAX as u64) as usize;
            let limit = input
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(if full_list { 50 } else { 200 })
                .clamp(1, 200) as usize;
            let mut listed = Vec::with_capacity(limit);
            for entry in catalog
                .into_iter()
                .filter(|note| in_scope(note.source_cwd.as_deref()))
                .skip(offset)
                .take(limit)
            {
                if full_list {
                    if let Some(note) = store.get_note(&entry.id)?
                        && in_scope(note.source_cwd.as_deref())
                    {
                        let mut value = json!(store.response_note(note));
                        value["noteId"] = json!(entry.id);
                        listed.push(value);
                    }
                } else {
                    listed.push(
                        json!({"id":numeric_id(&entry.id),"noteId":entry.id,"title":entry.title}),
                    );
                }
            }
            return Ok(json!(listed));
        }
        let native_id = input.get("noteId").and_then(Value::as_str);
        let legacy_id = input
            .get("id")
            .map(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_str()?.parse().ok())
                    .ok_or("id must be a numeric note ID")
            })
            .transpose()?;
        if native_id.is_some() && legacy_id.is_some() {
            return Err("Use id or noteId, not both".into());
        }
        let mut matching = catalog.iter().filter(|entry| {
            native_id.is_some_and(|id| entry.id == id)
                || legacy_id.is_some_and(|id| numeric_id(&entry.id) == id)
        });
        let existing = matching.next();
        if matching.next().is_some() {
            return Err(
                "Numeric note ID is ambiguous; use the exact noteId from notes.list".into(),
            );
        }
        let existing = existing
            .map(|entry| -> Result<_, String> {
                if !in_scope(entry.source_cwd.as_deref()) {
                    return Err("Note is outside the caller's assigned project".into());
                }
                let note = store
                    .get_note(&entry.id)?
                    .ok_or("Note was removed while reading")?;
                if !in_scope(note.source_cwd.as_deref()) {
                    return Err("Note is outside the caller's assigned project".into());
                }
                Ok(note)
            })
            .transpose()?;
        if action == "notes.read" {
            let note = existing.ok_or("Note not found in assigned projects")?;
            let note_id = note.id.clone();
            let mut value = json!(store.response_note(note));
            value["noteId"] = json!(note_id);
            return Ok(value);
        }
        if !write {
            return Err("Unsupported notes action".into());
        }
        if native_id.is_some() && existing.is_none() {
            return Err("Native noteId is unavailable; create without noteId".into());
        }
        let title = note_field(input, "title", vorkspace_core::notes::MAX_TITLE)?;
        let body = note_field(input, "body", 256 * 1024)?;
        let tags = input.get("tags").and_then(Value::as_array).map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        });
        let mut numeric = legacy_id.unwrap_or_else(vorkspace_core::time::now_ms);
        if existing.is_none() && native_id.is_none() {
            while catalog.iter().any(|entry| numeric_id(&entry.id) == numeric) {
                numeric = numeric.checked_add(1).ok_or("Could not allocate note ID")?;
            }
        }
        let note_id = existing
            .as_ref()
            .map(|note| note.id.clone())
            .unwrap_or_else(|| format!("gpui-note-{numeric}"));
        let blocked = self
            .page_tabs
            .notes
            .as_ref()
            .is_some_and(|view| view.read(cx).blocks_external_write(&note_id, cx))
            || self
                .page_tabs
                .note_tabs
                .values()
                .any(|view| view.read(cx).blocks_external_write(&note_id, cx));
        if blocked {
            return Err("The note has an open draft, recording, or save in progress; its editor was preserved".into());
        }
        // The dirty gate and native write share one UI turn. The store also
        // checks the exact saved note against concurrent writers.
        let saved = if let Some(note) = &existing {
            let update = NoteUpsert {
                id: note_id.clone(),
                title,
                body,
                tags: tags.unwrap_or_else(|| note.tags.clone()),
                ..Default::default()
            };
            store.update_note_preserving_artifact(update, note)?
        } else {
            let project = (projects.len() == 1)
                .then(|| projects[0].clone())
                .ok_or("Choose project when creating a note with several assigned projects")?;
            let update = NoteUpsert {
                id: note_id.clone(),
                title,
                body,
                tags: tags.unwrap_or_default(),
                source_session_id: store.persisted_session_id(caller).ok(),
                source_cwd: Some(project),
                finalize_slug: true,
            };
            store
                .upsert_note_if_current(&update, None)?
                .ok_or("The new note ID was taken. Retry to create another note.")?
        };
        let mut value = json!(store.response_note(saved));
        value["noteId"] = json!(note_id);
        drop(store);
        let refresh_if_clean = |view: &Entity<Notes>, cx: &mut Context<Self>| {
            let clean = view
                .read(cx)
                .selected_note_id()
                .is_none_or(|selected| !view.read(cx).blocks_external_write(&selected, cx));
            if clean {
                view.update(cx, |view, cx| view.refresh(cx));
            }
        };
        if let Some(view) = &self.page_tabs.notes {
            refresh_if_clean(view, cx);
        }
        for view in self.page_tabs.note_tabs.values() {
            refresh_if_clean(view, cx);
        }
        cx.notify();
        Ok(value)
    }
}

fn note_field(input: &Value, key: &str, max: usize) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| value.len() <= max)
        .map(str::to_owned)
        .ok_or_else(|| format!("Provide {key} (up to {max} bytes)."))
}
