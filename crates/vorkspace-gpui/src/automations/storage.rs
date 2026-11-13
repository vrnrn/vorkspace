//! Publish automation definitions and run receipts without replacing newer editor state.
use super::*;

impl Automations {
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_generation += 1;
        let generation = self.refresh_generation;
        let service = self.service.clone();
        let selected = self.selected.clone();
        let selected_for_work = selected.clone();
        let work = cx.background_executor().spawn(async move {
            let service = service
                .lock()
                .map_err(|_| "Automations unavailable".to_string())?;
            let list = service.list()?;
            let runs = selected_for_work
                .as_deref()
                .map(|id| service.runs(id))
                .transpose()?
                .unwrap_or_default();
            Ok::<_, String>((list, runs))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.refresh_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((list, runs)) => {
                        this.automations = list;
                        if this.selected == selected {
                            this.runs = runs;
                        }
                        if this
                            .selected
                            .as_ref()
                            .is_some_and(|id| !this.automations.iter().any(|a| &a.id == id))
                        {
                            this.selected = None;
                            this.runs.clear();
                            this.history = false;
                            this.confirm_delete = false;
                            if let Some(editor) = this.editor.as_mut().filter(|editor| editor.dirty) {
                                editor.draft.id = automations::new_id();
                                editor.saved_revision = DefinitionRevision::New;
                                this.error = Some("This automation was removed. Create a new automation to keep your edits.".into());
                            } else {
                                this.editor = None;
                                this.gallery = true;
                            }
                        }
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(draft) = self.collected(cx) else {
            return;
        };
        let Some(editor) = &self.editor else { return };
        let (identity, edits, revision) =
            (editor.name.entity_id(), editor.edits, editor.saved_revision);
        let definition = draft.id.clone();
        self.saving = true;
        self.error = None;
        let service = self.service.clone();
        let work = cx.background_executor().spawn(async move {
            service
                .lock()
                .map_err(|_| "Automations unavailable".to_string())?
                .save_definition(draft, revision)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                let editor = this.editor.as_mut().filter(|editor| {
                    editor.name.entity_id() == identity && editor.draft.id == definition
                });
                match result {
                    Ok(saved) => {
                        if let Some(editor) = editor {
                            editor.saved_revision = DefinitionRevision::Saved(saved.updated_at);
                            if editor.edits == edits {
                                editor.draft = AutomationUpsert::from_automation(&saved);
                                editor.dirty = false;
                            }
                            this.selected = Some(saved.id);
                        }
                        this.refresh(cx);
                    }
                    Err(error) if editor.is_some() => {
                        this.error = Some(error);
                        this.refresh(cx);
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn toggle(
        &mut self,
        id: String,
        revision: i64,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.error = None;
        let service = self.service.clone();
        let work = cx.background_executor().spawn(async move {
            service
                .lock()
                .map_err(|_| "Automations unavailable".to_string())?
                .set_enabled(&id, revision, enabled)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(saved) => {
                        if let Some(editor) = &mut this.editor
                            && editor.draft.id == saved.id
                            && editor.saved_revision == DefinitionRevision::Saved(revision)
                            && !editor.dirty
                        {
                            editor.draft.enabled = saved.enabled;
                            editor.saved_revision = DefinitionRevision::Saved(saved.updated_at);
                        }
                    }
                    Err(error) => this.error = Some(error),
                }
                this.refresh(cx);
            });
        })
        .detach();
    }
    pub(super) fn run(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some(editor) = self.editor.as_ref().filter(|editor| !editor.dirty) else {
            return;
        };
        let DefinitionRevision::Saved(revision) = editor.saved_revision else {
            return;
        };
        let id = editor.draft.id.clone();
        self.running = true;
        self.error = None;
        let service = self.service.clone();
        let work = cx.background_executor().spawn(async move {
            service
                .lock()
                .map_err(|_| "Automations unavailable".to_string())?
                .run_now(&id, revision, vorkspace_core::time::now_ms() as i64)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.running = false;
                match result {
                    Ok(run) => cx.emit(Event::Launch(Box::new(run))),
                    Err(error) => this.error = Some(error),
                }
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn delete(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else { return };
        let DefinitionRevision::Saved(revision) = editor.saved_revision else {
            return;
        };
        if !std::mem::take(&mut self.confirm_delete) {
            return;
        }
        let id = editor.draft.id.clone();
        let deleting = id.clone();
        let origin = (editor.name.entity_id(), editor.edits);
        cx.notify();
        let service = self.service.clone();
        let work = cx.background_executor().spawn(async move {
            service
                .lock()
                .map_err(|_| "Automations unavailable".to_string())?
                .delete_definition(&id, revision)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(())
                        if this.editor.as_ref().is_some_and(|editor| {
                            (editor.name.entity_id(), editor.edits) == origin
                                && editor.draft.id == deleting
                        }) =>
                    {
                        this.editor = None;
                        this.selected = None;
                        this.gallery = true;
                        this.runs.clear();
                    }
                    Err(error) if this.selected.as_deref() == Some(&deleting) => {
                        this.error = Some(error)
                    }
                    _ => {}
                }
                this.refresh(cx);
            });
        })
        .detach();
    }
}
