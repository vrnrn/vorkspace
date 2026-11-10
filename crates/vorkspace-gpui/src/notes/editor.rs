//! Note editing, attachment admission and revision-checked saves.
use super::*;

impl Notes {
    pub(super) fn draft(&self, cx: &App) -> Option<Note> {
        let mut note = self
            .notes
            .iter()
            .find(|note| Some(&note.id) == self.selected.as_ref())?
            .clone();
        note.title = self.title.read(cx).content.to_string();
        note.body = self.body.read(cx).content.to_string();
        Some(note)
    }

    /// `Input::set_text` emits `Changed` for programmatic loads too, and those
    /// arrive after `load_selection` has cleared `dirty`. Only content that
    /// differs from the saved note is a user edit; otherwise an empty or
    /// freshly loaded view would stay dirty with nothing to save, and
    /// create/select/refresh would all refuse to proceed.
    pub(super) fn input_changed(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft(cx) else {
            return;
        };
        let unchanged = self
            .notes
            .iter()
            .find(|note| note.id == draft.id)
            .is_some_and(|saved| saved.title == draft.title && saved.body == draft.body);
        if !unchanged {
            self.edited(cx);
        }
    }

    pub(super) fn edited(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        if self.save_conflict {
            cx.notify();
            return;
        }
        self.error = None;
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = this.update(cx, |this, cx| this.persist(false, cx));
        }));
        cx.notify();
    }

    /// Serializing at the editor boundary means a delayed save never overwrites
    /// a newer edit or resurrects a deleted note.
    pub(crate) fn flush(&mut self, cx: &mut Context<Self>) {
        self.add_tag(cx);
        self.persist(true, cx);
        self.recording.update(cx, |page, cx| page.flush_note(cx));
    }

    /// Preflight a close before Shell releases this editor entity. Failed note
    /// writes and unfinished recording saves retain the same draft owner.
    pub(crate) fn prepare_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.flush(cx);
        if self.dirty || self.image_note.is_some() {
            return false;
        }
        self.recording
            .update(cx, |page, cx| page.prepare_note_close(cx))
    }

    pub(super) fn persist(&mut self, finalize: bool, cx: &mut Context<Self>) {
        self.debounce = None;
        if self.save_conflict || self.image_note.is_some() && self.image_note == self.selected {
            return;
        }
        let Some(mut draft) = self.draft(cx) else {
            return;
        };
        if self.ephemeral.contains(&draft.id)
            && draft.title.trim().is_empty()
            && draft.body.trim().is_empty()
            && draft.tags.is_empty()
        {
            self.dirty = false;
            return;
        }
        let saved_title = self
            .notes
            .iter()
            .find(|note| note.id == draft.id)
            .map(|note| note.title.clone())
            .unwrap_or_default();
        if !self.dirty && !(finalize && (draft.slug_pending || draft.title != saved_title)) {
            return;
        }
        self.dirty = true;
        if draft.title.trim().is_empty() {
            draft.title = if finalize {
                notes::title(&draft.body)
            } else {
                saved_title
            };
        }
        let result = locked(&self.store).and_then(|store| {
            store.upsert_note_if_current(&draft.upsert(finalize), self.saved_note.as_ref())
        });
        match result {
            Ok(Some(saved)) => {
                self.ephemeral.remove(&saved.id);
                self.revision = self.revision.wrapping_add(1);
                if finalize {
                    self.title
                        .update(cx, |input, cx| input.set_text(saved.title.clone(), cx));
                }
                self.notes.retain(|note| note.id != saved.id);
                self.saved_note = Some(saved.clone());
                self.notes.push(saved);
                self.notes
                    .sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
                self.dirty = false;
                self.error = None;
            }
            Ok(None) => {
                self.save_conflict = true;
                self.error = Some("This note changed elsewhere. Your edits are still here.".into());
            }
            Err(error) => self.error = Some(format!("Could not save note: {error}")),
        }
        cx.notify();
    }

    pub(super) fn resolve_save_conflict(&mut self, save_copy: bool, cx: &mut Context<Self>) {
        if !self.save_conflict {
            return;
        }
        if save_copy {
            self.add_tag(cx);
        }
        let Some(draft) = self.draft(cx) else {
            return;
        };
        let result = locked(&self.store).and_then(|store| {
            let latest = store.get_note(&draft.id)?;
            let copy = if save_copy {
                let mut copy = draft.upsert(true);
                copy.id = format!("note-{}", uuid::Uuid::new_v4().simple());
                let title = if draft.title.trim().is_empty() {
                    notes::title(&draft.body)
                } else {
                    draft.title.clone()
                };
                copy.title = format!("{title} (copy)");
                Some(store.copy_note_draft(&draft.id, &copy)?)
            } else {
                None
            };
            Ok((latest, copy))
        });
        match result {
            Ok((latest, copy)) => {
                self.notes.retain(|note| note.id != draft.id);
                let selected = if latest.is_some() || self.pinned.is_some() {
                    Some(draft.id)
                } else {
                    None
                };
                self.notes.extend(latest);
                self.load_selection(selected, cx);
                self.error = None;
                if let Some((copy, missing)) = copy {
                    let id = copy.id.clone();
                    self.notes.insert(0, copy);
                    cx.emit(Event::OpenTab(id));
                    if missing > 0 {
                        cx.emit(Event::Notice(format!(
                            "Saved a copy. {missing} original image(s) could not be recovered."
                        )));
                    }
                }
            }
            Err(error) => self.error = Some(format!("Could not resolve the note edit: {error}")),
        }
        cx.notify();
    }

    pub(super) fn update_preview(&mut self, cx: &mut Context<Self>) {
        self.document = self.draft(cx).map(|note| {
            cx.new(|cx| {
                Document::note(
                    without_title_heading(&note.body, &note.title),
                    note.source_cwd.as_deref().unwrap_or(&self.cwd),
                    &self.store,
                    cx,
                )
            })
        });
    }

    pub(super) fn add_tag(&mut self, cx: &mut Context<Self>) {
        let text = self
            .tag
            .read(cx)
            .content
            .trim()
            .trim_end_matches(',')
            .to_owned();
        if text.is_empty() {
            return;
        }
        self.tag
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        if let Some(note) = self
            .notes
            .iter_mut()
            .find(|note| Some(&note.id) == self.selected.as_ref())
        {
            let tags = notes::normalize_tags(&[note.tags.clone(), vec![text]].concat());
            if tags != note.tags {
                note.tags = tags;
                self.edited(cx);
            }
        }
    }

    pub(super) fn remove_tag(&mut self, tag: &str, cx: &mut Context<Self>) {
        if let Some(note) = self
            .notes
            .iter_mut()
            .find(|note| Some(&note.id) == self.selected.as_ref())
        {
            note.tags.retain(|saved| saved != tag);
            self.edited(cx);
        }
    }

    pub(super) fn move_project(&mut self, path: String, cx: &mut Context<Self>) {
        if let Some(note) = self
            .notes
            .iter_mut()
            .find(|note| Some(&note.id) == self.selected.as_ref())
        {
            note.source_cwd = Some(path);
            self.revision = self.revision.wrapping_add(1);
            self.dirty = true;
            self.persist(false, cx);
            self.update_preview(cx);
        }
        self.project_open = false;
        cx.notify();
    }

    pub(super) fn add_images(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() || self.image_note.is_some() {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        let range = if self.source {
            self.body.read(cx).selection_range()
        } else {
            let length = self.body.read(cx).content.len();
            length..length
        };
        let store = self.store.clone();
        let draft = self
            .draft(cx)
            .filter(|note| self.ephemeral.contains(&note.id))
            .map(|note| note.upsert(false));
        self.image_note = Some(id.clone());
        self.revision = self.revision.wrapping_add(1);
        self.image_drag = false;
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            let creates_note = draft.is_some() && store.get_note(&id)?.is_none();
            let mut images = Vec::new();
            let mut failure = None;
            for path in paths {
                match store.save_note_image(&id, &path, draft.as_ref()) {
                    Ok(image) => images.push(image),
                    Err(error) => failure = Some(error),
                }
            }
            if images.is_empty() {
                return Err(
                    failure.unwrap_or_else(|| "Drop a PNG, JPG, GIF, WebP, or SVG image.".into())
                );
            }
            let staged = if creates_note {
                store.get_note(&id)?
            } else {
                None
            };
            Ok((id, images, staged))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.image_note = None;
                match result {
                    Ok((id, images, staged)) => {
                        this.ephemeral.remove(&id);
                        let block = images
                            .iter()
                            .map(notes::image_markdown)
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        if this.selected.as_deref() == Some(&id) {
                            if staged.is_some() {
                                this.saved_note = staged;
                            }
                            let text = this.body.read(cx).content.to_string();
                            let from = range.start.min(text.len());
                            let to = range.end.max(from).min(text.len());
                            let range = if text.is_char_boundary(from) && text.is_char_boundary(to)
                            {
                                from..to
                            } else {
                                text.len()..text.len()
                            };
                            let before = &text[..range.start];
                            let after = &text[range.end..];
                            let leading = if before.is_empty() || before.ends_with("\n\n") {
                                ""
                            } else if before.ends_with('\n') {
                                "\n"
                            } else {
                                "\n\n"
                            };
                            let trailing = if after.is_empty() || after.starts_with("\n\n") {
                                ""
                            } else if after.starts_with('\n') {
                                "\n"
                            } else {
                                "\n\n"
                            };
                            let insert = format!("{leading}{block}{trailing}");
                            let cursor = leading.len() + block.len();
                            this.body.update(cx, |input, cx| {
                                input.replace_bytes(range, &insert, cursor, cx)
                            });
                            this.edited(cx);
                            this.update_preview(cx);
                            if this.source {
                                window.focus(&this.body.focus_handle(cx), cx);
                            }
                        } else if let Some(note) = this.notes.iter_mut().find(|note| note.id == id)
                        {
                            // Append to the current durable note after navigation.
                            // The first image of a new draft is already saved.
                            let result = locked(&this.store).and_then(|store| {
                                let latest = store.get_note(&id)?.ok_or("Note no longer exists")?;
                                let mut update = latest.upsert(false);
                                for image in images.iter().skip(usize::from(staged.is_some())) {
                                    if !update.body.is_empty() && !update.body.ends_with("\n\n") {
                                        update.body.push_str("\n\n");
                                    }
                                    update.body.push_str(&notes::image_markdown(image));
                                }
                                if update.body == latest.body {
                                    return Ok(latest);
                                }
                                store
                                    .upsert_note_if_current(&update, Some(&latest))?
                                    .ok_or_else(|| "The note changed while adding images.".into())
                            });
                            match result {
                                Ok(saved) => {
                                    this.revision = this.revision.wrapping_add(1);
                                    *note = saved;
                                }
                                Err(error) => this.error = Some(error),
                            }
                        }
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
