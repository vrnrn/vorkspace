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
        if self.document.as_ref().is_some_and(|doc| doc.id() == id)
            && !self.opening
            && self.next_open.is_none()
        {
            return;
        }
        self.navigate(
            if recording_id.is_some() {
                Action::Open(id)
            } else {
                Action::NewDraft(id)
            },
            cx,
        );
    }

    pub(crate) fn is_for_note(&self, note_id: &str) -> bool {
        self.note_ids
            .get(note_id)
            .is_some_and(|id| self.document.as_ref().is_some_and(|doc| doc.id() == id))
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
        self.navigate(
            Action::Section(if transcript {
                Section::Transcript
            } else {
                Section::Summary
            }),
            cx,
        );
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
