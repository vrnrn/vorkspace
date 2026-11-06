//! Recording is an attachment to a note; Notes owns navigation and note text.
use super::{
    Event, Page,
    document::{Action, Document, Section},
    view,
};
use crate::controls::page;
use gpui::{AnyElement, Context, div, prelude::*, px};
use vorkspace_runtime::runtime::locked;
use vorkspace_ui::theme::{StatusColor, content, icon, status};

pub(super) struct PendingNote {
    recording_id: String,
    saved: bool,
    section: Option<Section>,
}

impl Page {
    pub(crate) fn show_note(
        &mut self,
        note_id: &str,
        recording_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let id = recording_id.map(str::to_owned).unwrap_or_else(|| {
            self.note_ids
                .entry(note_id.into())
                .or_insert_with(|| uuid::Uuid::new_v4().simple().to_string())
                .clone()
        });
        self.note_ids.insert(note_id.into(), id.clone());
        if self
            .pending_note
            .as_ref()
            .is_some_and(|pending| pending.recording_id == id)
        {
            // Refreshes retain the same identity and explicit tab through
            // saving, queued I/O and loading. Failed work can be retried.
            self.resume_note_selection(cx);
            return;
        }
        if self.document.as_ref().is_some_and(|doc| doc.id() == id)
            && self.pending_note.is_none()
            && self.opening.is_none()
            && self.next_open.is_none()
        {
            return;
        }
        // Notes is the selection authority; keep its identity and tab together
        // until installation, even when a preceding operation fails or saves.
        self.intent += 1;
        self.open_epoch += 1;
        self.after_save = None;
        self.next_open = None;
        self.pending_note = Some(PendingNote {
            recording_id: id,
            saved: recording_id.is_some(),
            section: None,
        });
        self.resume_note_selection(cx);
    }

    pub(crate) fn is_for_note(&self, note_id: &str) -> bool {
        self.note_ids
            .get(note_id)
            .is_some_and(|id| self.document.as_ref().is_some_and(|doc| doc.id() == id))
    }

    pub(crate) fn has_pending_note_edit(&self, note_id: &str) -> bool {
        let Some(id) = self.note_ids.get(note_id) else {
            return false;
        };
        self.drafts.pending_for(id)
            || self.preparing.as_deref() == Some(id.as_str())
            || self
                .document
                .as_ref()
                .is_some_and(|doc| doc.id() == id.as_str() && (doc.dirty() || self.saving))
    }

    /// Supersede the current note when its Notes pane has no selection. Use
    /// the normal note transition so a dirty recording is saved before its
    /// edit lease is released, including when a save is already in flight.
    pub(crate) fn clear_note(&mut self, cx: &mut Context<Self>) {
        if self.document.is_none() && self.pending_note.is_none() {
            return;
        }
        let detached = format!("detached-notes-{}", uuid::Uuid::new_v4().simple());
        self.show_note(&detached, None, cx);
        // This inert destination is never exposed as a note or recording.
        self.note_ids.remove(&detached);
    }

    pub(super) fn resume_note_selection(&mut self, cx: &mut Context<Self>) {
        if !self.enabled || self.deleting || self.discarding || self.saving {
            return;
        }
        let Some(pending) = &self.pending_note else {
            return;
        };
        if self
            .document
            .as_ref()
            .is_some_and(|doc| doc.id() == pending.recording_id)
        {
            self.finish_note_selection();
        } else if self.document.as_ref().is_some_and(Document::dirty) {
            self.save(cx);
        } else if !self.opening_document(&pending.recording_id) {
            let id = pending.recording_id.clone();
            self.confirm_delete = false;
            self.source_open = false;
            if pending.saved {
                self.open(id, cx);
            } else {
                self.install(Some(Document::temporary_with_id(id)), cx);
            }
            self.finish_note_selection();
            cx.notify();
        }
    }

    pub(super) fn finish_note_selection(&mut self) {
        if self.pending_note.as_ref().is_some_and(|pending| {
            self.document
                .as_ref()
                .is_some_and(|doc| doc.id() == pending.recording_id)
        }) && let Some(pending) = self.pending_note.take()
            && let Some(section) = pending.section
        {
            self.section = section;
            self.pending_focus = None;
        }
    }

    pub(crate) fn transcript_selected(&self) -> bool {
        self.section == Section::Transcript
    }

    pub(crate) fn selected_text(&self) -> Option<&str> {
        let doc = self.document.as_ref()?;
        if self.section == Section::Transcript {
            // The live presentation contains provisional segments; sharing
            // waits for its final transcript instead of exporting stale text.
            if doc.state.phase.active() || doc.awaiting_transcript {
                return None;
            }
            Some(doc.text(super::document::Field::Transcript))
        } else {
            Some(doc.text(super::document::Field::Summary))
        }
    }

    pub(crate) fn hide_note_content(&mut self, cx: &mut Context<Self>) {
        // A deliberate move to authored note text supersedes automatic
        // summary navigation, while generation itself may finish and save.
        self.intent += 1;
        self.pending_focus = None;
        self.flush_note(cx);
    }

    pub(crate) fn flush_note(&mut self, cx: &mut Context<Self>) {
        if self.document.as_ref().is_some_and(Document::dirty) {
            self.save(cx);
        }
    }

    /// A note pane may close while capture continues in the app-wide owner.
    /// Keep the pane only while its local edit or checkpoint still needs it.
    pub(crate) fn prepare_note_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.flush_note(cx);
        if self.saving || self.document.as_ref().is_some_and(Document::dirty) {
            return false;
        }
        if !self.drafts.has_pending() {
            return true;
        }
        match self.meetings.clone() {
            Ok(meetings) => match self.drafts.flush(&meetings) {
                Ok(()) => true,
                Err(error) => {
                    self.fail(error, cx);
                    false
                }
            },
            Err(error) => {
                self.fail(error, cx);
                false
            }
        }
    }

    pub(super) fn attach_note(&self, id: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let Some(store) = &self.note_store else {
            return Ok(());
        };
        let note_id = self
            .note_ids
            .iter()
            .find_map(|(note, recording)| (recording == id).then_some(note))
            .ok_or("Choose a note before recording.")?;
        locked(store)?.attach_note_recording(note_id, id)?;
        cx.emit(Event::Attached {
            note_id: note_id.clone(),
            recording_id: id.into(),
        });
        Ok(())
    }

    pub(crate) fn note_section(&mut self, transcript: bool, cx: &mut Context<Self>) {
        let section = if transcript {
            Section::Transcript
        } else {
            Section::Summary
        };
        if let Some(pending) = &mut self.pending_note {
            pending.section = Some(section);
            self.section = section;
            self.pending_focus = None;
            self.intent += 1;
            cx.notify();
            return;
        }
        self.navigate(Action::Section(section), cx);
    }

    pub(crate) fn can_delete_note(&self) -> bool {
        self.editable()
            && !self.saving
            && !self.deleting
            && !self.discarding
            && self.preparing.is_none()
            && self.analysis.is_none()
            && self
                .document
                .as_ref()
                .is_some_and(|doc| !doc.state.phase.active() && !doc.awaiting_transcript)
    }

    pub(crate) fn delete_note_recording(&mut self, cx: &mut Context<Self>) {
        if !self.can_delete_note() {
            return;
        }
        self.confirm_delete = true;
        self.delete(cx);
    }

    pub(super) fn emit_note_deleted(&self, id: &str, cx: &mut Context<Self>) {
        if let Some(note_id) = self
            .note_ids
            .iter()
            .find_map(|(note, recording)| (recording == id).then_some(note))
        {
            cx.emit(Event::Deleted(note_id.clone()));
        }
    }

    pub(crate) fn note_controls(
        &self,
        extra: AnyElement,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(doc) = &self.document else {
            return page::meta().child("Loading recording…").into_any_element();
        };
        let saved = doc.saved.as_ref();
        let summary = saved.and_then(|saved| saved.meeting.summary.as_ref());
        let busy = self.analysis.as_ref().is_some_and(|job| job.id == doc.id());
        let can_summarize = !doc.state.phase.active()
            && !doc.awaiting_transcript
            && self.analysis.is_none()
            && !self.deleting
            && !self.discarding
            && !self.saving
            && !doc
                .text(super::document::Field::Transcript)
                .trim()
                .is_empty();
        let writable = self.editable();
        let label = if busy { "Summarizing…" } else { "Summarize" };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(self.record_control(doc, cx))
                    .when(!doc.unrecorded() || saved.is_some(), |d| {
                        d.child(
                            page::secondary("note-summarize", label, false, !can_summarize)
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if can_summarize {
                                        this.navigate(Action::Summarize, cx);
                                    }
                                })),
                        )
                    })
                    .child(self.source_picker(doc, writable && !self.saving, cx))
                    .child(extra)
                    .when(doc.dirty(), |d| {
                        d.child(
                            page::secondary(
                                "note-save-recording",
                                "Save transcript edits",
                                false,
                                !writable || self.saving,
                            )
                            .child(if self.saving {
                                "Saving…"
                            } else {
                                "Save edits"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        )
                        .child(
                            page::action(
                                "note-discard-recording",
                                "Discard transcript edits",
                                writable && !self.saving,
                            )
                            .child("Discard edits")
                            .on_click(cx.listener(|this, _, _, cx| this.discard(cx))),
                        )
                    })
                    .when(
                        visible && summary.is_some() && self.section != Section::Transcript,
                        |d| {
                            d.child(
                                page::action("note-edit-summary", "Edit summary", writable)
                                    .child(if self.section == Section::Source {
                                        "View summary"
                                    } else {
                                        "Edit summary"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let next = if this.section == Section::Source {
                                            Section::Summary
                                        } else {
                                            Section::Source
                                        };
                                        this.navigate(Action::Section(next), cx);
                                    })),
                            )
                        },
                    ),
            )
            .when(!doc.unrecorded(), |d| {
                d.child(page::meta().child(view::metadata(&doc.state)))
            })
            .when_some(doc.draft_status(), |d, text| {
                d.child(page::meta().child(text))
            })
            .when_some(self.notice.clone(), |d, text| {
                d.child(page::meta().child(text))
            })
            .when_some(
                self.error.clone().or_else(|| doc.state.error.clone()),
                |d, text| {
                    d.child(
                        page::meta()
                            .text_color(status(StatusColor::Danger, 1.))
                            .child(text),
                    )
                },
            )
            .when(
                saved.is_some_and(|saved| {
                    summary.is_some_and(|summary| {
                        summary.source_revision != saved.meeting.transcript_revision
                    })
                }),
                |d| {
                    d.child(page::meta().child(
                        "The transcript has changed. Summarize again to update these notes.",
                    ))
                },
            )
            .when(self.source_open && writable, |d| {
                d.child(self.source_menu(doc, cx))
            })
            .into_any_element()
    }

    pub(crate) fn note_content(&self) -> AnyElement {
        let Some(doc) = &self.document else {
            return page::meta().child("Loading recording…").into_any_element();
        };
        let live = self.live.as_ref().filter(|live| {
            live.state.id == doc.id() && (live.state.phase.active() || doc.awaiting_transcript)
        });
        let content = match self.section {
            Section::Transcript => match live {
                Some(live) => view::live_transcript(live),
                None => view::editor("Note transcript", &self.transcript),
            },
            Section::Source => view::editor("Meeting notes Markdown", &self.summary),
            Section::Summary => div()
                .children(self.preview.clone())
                .when(self.preview.is_none(), |d| {
                    d.child(page::empty(
                        icon("note", 24.),
                        "No summary yet",
                        "Record a meeting, then choose Summarize.",
                    ))
                })
                .text_color(content(1.))
                .into_any_element(),
        };
        div()
            .when(self.section != Section::Summary, |d| d.h(px(520.)))
            .min_h(px(240.))
            .min_w_0()
            .child(content)
            .into_any_element()
    }
}
