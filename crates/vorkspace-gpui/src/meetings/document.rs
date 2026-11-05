//! Document identity, revision checks, and recovery checkpoints.
use super::{Event, Page};
use crate::markdown;
use gpui::{AppContext, Context};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use vorkspace_meetings::{
    DraftWrite, EditLease, Meeting, Meetings,
    recording::{MAX_TRANSCRIPT, State},
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum DraftView {
    #[default]
    Transcript,
    Summary,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Section {
    #[default]
    Transcript,
    Summary,
    Source,
}

pub(super) struct PendingOpen {
    epoch: u64,
    intent: u64,
    id: String,
    retry: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Transcript,
    Summary,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Draft {
    revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    view: DraftView,
    title_focused: bool,
    start: u32,
    end: u32,
}

impl Draft {
    fn text(&self, field: Field) -> Option<&str> {
        match field {
            Field::Transcript => self.transcript.as_deref(),
            Field::Summary => self.summary.as_deref(),
        }
    }

    fn set_text(&mut self, field: Field, text: String) {
        *match field {
            Field::Transcript => &mut self.transcript,
            Field::Summary => &mut self.summary,
        } = Some(text);
    }
}

#[derive(Deserialize)]
pub(super) struct Snapshot {
    pub meeting: Meeting,
    pub state: State,
    pub transcript: String,
    #[serde(default)]
    pub sequence: u64,
    draft: Option<Draft>,
    pub(super) draft_version: Option<String>,
    draft_error: Option<String>,
}

pub(super) struct Document {
    pub state: State,
    pub saved: Option<Snapshot>,
    pub lease: Option<Arc<EditorLease>>,
    pub awaiting_transcript: bool,
    draft: Option<Draft>,
    version: Option<String>,
    checkpoint_error: Option<String>,
}

impl Document {
    fn new(mut snapshot: Snapshot, lease: Option<Arc<EditorLease>>) -> Self {
        Self {
            state: snapshot.state.clone(),
            lease,
            awaiting_transcript: false,
            draft: snapshot.draft.take(),
            version: snapshot.draft_version.clone(),
            checkpoint_error: snapshot.draft_error.take(),
            saved: Some(snapshot),
        }
    }

    pub(super) fn live(state: State) -> Self {
        Self {
            state,
            saved: None,
            lease: None,
            awaiting_transcript: false,
            draft: None,
            version: None,
            checkpoint_error: None,
        }
    }

    pub(super) fn temporary_with_id(id: String) -> Self {
        Self {
            lease: Some(Arc::default()),
            ..Self::live(State::dormant(id))
        }
    }

    pub(super) fn unrecorded(&self) -> bool {
        self.state.started_at == 0 && self.state.run.is_none()
    }

    pub(super) fn has_content(&self) -> bool {
        // Older recording drafts may still carry a title. Preserve their
        // meaningful content even though Notes now owns title editing.
        let title = self
            .draft
            .as_ref()
            .and_then(|draft| draft.title.as_deref())
            .unwrap_or_else(|| self.saved.as_ref().map_or("", |saved| &saved.meeting.title));
        !title.trim().is_empty() || !self.text(Field::Transcript).trim().is_empty()
    }

    pub(super) fn id(&self) -> &str {
        &self.state.id
    }

    pub(super) fn editable(&self) -> bool {
        self.lease.is_some()
            && !self.awaiting_transcript
            && !self.state.phase.active()
            && (self.saved.is_some() || self.unrecorded())
    }

    pub(super) fn text(&self, field: Field) -> &str {
        if let Some(text) = self.draft.as_ref().and_then(|draft| draft.text(field)) {
            return text;
        }
        match field {
            Field::Transcript => self.saved.as_ref().map_or("", |saved| &saved.transcript),
            Field::Summary => self
                .saved
                .as_ref()
                .and_then(|saved| saved.meeting.summary.as_ref())
                .map_or("", |summary| &summary.text),
        }
    }

    pub(super) fn dirty(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn conflict(&self) -> bool {
        self.draft
            .as_ref()
            .zip(self.saved.as_ref())
            .is_some_and(|(draft, saved)| draft.revision != saved.meeting.revision)
    }

    pub(super) fn draft_status(&self) -> Option<String> {
        if let Some(error) = &self.checkpoint_error {
            Some(format!("Could not checkpoint your draft. {error}"))
        } else if self.conflict() {
            Some(
                "This draft is based on an older version. Discard it to reload the saved meeting."
                    .into(),
            )
        } else if self.needs_edit_lease() {
            Some("This meeting is open for editing in another window.".into())
        } else {
            None
        }
    }

    pub(super) fn needs_edit_lease(&self) -> bool {
        self.saved.is_some() && self.lease.is_none() && !self.state.phase.active()
    }

    fn write(&self) -> Result<DraftWrite, String> {
        DraftWrite::from_params(json!({
            "id": self.id(),
            "version": self.version,
            "expected": self.saved.as_ref().and_then(|saved| saved.draft_version.as_deref()),
            "draft": self.draft,
        }))
    }

    fn apply_save(&mut self, meeting: Meeting, version: String, submitted: Draft) {
        let saved = self.saved.get_or_insert_with(|| Snapshot {
            meeting: meeting.clone(),
            state: self.state.clone(),
            transcript: String::new(),
            sequence: 0,
            draft: None,
            draft_version: None,
            draft_error: None,
        });
        if meeting.revision < saved.meeting.revision {
            return;
        }
        saved.draft_version = Some(version.clone());
        if let Some(text) = submitted.transcript {
            saved.transcript = text;
        }
        saved.meeting = meeting;
        if self.version.as_ref() == Some(&version) {
            self.draft = None;
            self.checkpoint_error = None;
        } else if let Some(draft) = &mut self.draft
            && draft.revision == submitted.revision
            && saved.meeting.revision == submitted.revision + 1
        {
            // The durable save receipt applies the same rebase if the newer
            // checkpoint reaches disk before this completion reaches the view.
            draft.revision = saved.meeting.revision;
        }
    }
}

pub(super) enum Action {
    Section(Section),
    Record(String),
    Summarize,
}

/// A new, empty editor claims its storage lease on the first meaningful
/// checkpoint or Start. The editor and admitted disk work share the lease.
#[derive(Default)]
pub(super) struct EditorLease(Mutex<Option<EditLease>>);

impl EditorLease {
    pub(super) fn materialize(&self, meetings: &Meetings, id: &str) -> Result<(), String> {
        let mut lease = self.0.lock().unwrap();
        if lease.is_none() {
            let acquired = meetings
                .try_edit(id)?
                .ok_or("This meeting is open for editing in another window.")?;
            meetings.prepare_document(id)?;
            *lease = Some(acquired);
        }
        Ok(())
    }
}

struct PendingDraft {
    write: DraftWrite,
    lease: Arc<EditorLease>,
    skip_unmaterialized: bool,
}

#[derive(Default)]
pub(super) struct DraftBuffer {
    pending: Mutex<Option<Arc<PendingDraft>>>,
    writer: Mutex<()>,
}

impl DraftBuffer {
    fn stage(
        &self,
        write: DraftWrite,
        lease: Arc<EditorLease>,
        skip_unmaterialized: bool,
    ) -> Result<(), String> {
        let mut pending = self.pending.lock().unwrap();
        if let Some(old) = pending.as_ref() {
            if old.write.id == write.id && old.write.version == write.version {
                return Ok(());
            }
            if old.write.id != write.id || !write.follows(&old.write.version) {
                return Err(
                    "Another meeting draft is still being checkpointed. Try again shortly.".into(),
                );
            }
        }
        *pending = Some(Arc::new(PendingDraft {
            write,
            lease,
            skip_unmaterialized,
        }));
        Ok(())
    }

    fn has_pending(&self) -> bool {
        self.pending.lock().unwrap().is_some()
    }

    pub(super) fn flush(&self, meetings: &Meetings) -> Result<(), String> {
        let _writer = self.writer.lock().unwrap();
        let Some(edit) = self.pending.lock().unwrap().clone() else {
            return Ok(());
        };
        let materialized = edit.lease.0.lock().unwrap().is_some();
        if !edit.skip_unmaterialized || materialized {
            edit.lease.materialize(meetings, &edit.write.id)?;
            meetings.checkpoint_draft(&edit.write)?;
        }
        let mut pending = self.pending.lock().unwrap();
        if pending
            .as_ref()
            .is_some_and(|latest| Arc::ptr_eq(latest, &edit))
        {
            *pending = None;
        }
        Ok(())
    }

    fn clear(&self, id: &str) {
        let mut pending = self.pending.lock().unwrap();
        if pending.as_ref().is_some_and(|edit| edit.write.id == id) {
            *pending = None;
        }
    }

    fn open(
        &self,
        meetings: &Meetings,
        id: &str,
        previous: Option<Arc<EditorLease>>,
    ) -> Result<Document, String> {
        let lease = match previous {
            Some(lease) => Some(lease),
            None => meetings
                .try_edit(id)?
                .map(|lease| Arc::new(EditorLease(Mutex::new(Some(lease))))),
        };
        if lease.is_none() {
            return decode(meetings.read(id)?).map(|saved| Document::new(saved, None));
        }
        let checkpoint = self.flush(meetings);
        let mut snapshot = meetings.open_document(id)?;
        if let Err(error) = checkpoint
            && let Some(edit) = self
                .pending
                .lock()
                .unwrap()
                .as_ref()
                .filter(|edit| edit.write.id == id)
        {
            edit.write.restore_pending(&mut snapshot, error)?;
        }
        decode(snapshot).map(|snapshot| Document::new(snapshot, lease))
    }
}

pub(super) fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

impl Page {
    pub(super) fn edit(&mut self, field: Field, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        let input = self.input(field).read(cx);
        let text = input.content.clone();
        let doc = self.document.as_mut().unwrap();
        if text.as_ref() == doc.text(field) {
            return;
        }
        let selection = input.selection_utf16();
        let oversized = match field {
            Field::Transcript => text.len() > MAX_TRANSCRIPT,
            Field::Summary => text.len() > 64 * 1024,
        };
        if oversized {
            let previous = doc.text(field).to_owned();
            self.input(field).update(cx, |input, cx| {
                input.set_text(previous, cx);
                input.select_utf16(selection, cx);
            });
            self.fail("This edit exceeds the meeting's size limit.".into(), cx);
            return;
        }
        self.intent += 1;
        self.open_epoch += 1;
        self.error = None;
        let draft = doc.draft.get_or_insert_with(|| Draft {
            revision: doc.saved.as_ref().map_or(0, |saved| saved.meeting.revision),
            title: None,
            transcript: None,
            summary: None,
            view: DraftView::Transcript,
            title_focused: false,
            start: 0,
            end: 0,
        });
        draft.set_text(field, text.to_string());
        draft.view = if self.section == Section::Transcript {
            DraftView::Transcript
        } else {
            DraftView::Summary
        };
        draft.title_focused = false;
        draft.start = selection.start as u32;
        draft.end = selection.end as u32;
        self.draft_sequence += 1;
        doc.version = Some(format!("{}{:08x}", self.draft_prefix, self.draft_sequence));
        match doc.write().and_then(|write| {
            self.drafts.stage(
                write,
                doc.lease.clone().unwrap(),
                doc.unrecorded() && !doc.has_content(),
            )
        }) {
            Ok(()) => self.schedule_draft(cx),
            Err(error) => doc.checkpoint_error = Some(error),
        }
        cx.notify();
    }

    fn schedule_draft(&mut self, cx: &mut Context<Self>) {
        if self.draft_timer.is_some() || !self.drafts.has_pending() {
            return;
        }
        let Ok(meetings) = self.meetings.clone() else {
            return;
        };
        let drafts = self.drafts.clone();
        self.draft_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            let Ok(version) = this.update(cx, |this, _| {
                this.document.as_ref().and_then(|doc| doc.version.clone())
            }) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { drafts.flush(&meetings) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.draft_timer = None;
                if let Some(doc) = &mut this.document
                    && doc.dirty()
                    && doc.version == version
                {
                    doc.checkpoint_error = result.err();
                    cx.notify();
                } else {
                    // A newer edit arrived during I/O. Give it its own checkpoint.
                    this.schedule_draft(cx);
                }
            });
        }));
    }

    pub(super) fn navigate(&mut self, action: Action, cx: &mut Context<Self>) {
        if !self.enabled || self.deleting || self.discarding {
            return;
        }
        self.intent += 1;
        if matches!(&action, Action::Section(section) if *section == self.section) {
            return;
        }
        if self.document.as_ref().is_some_and(Document::dirty) {
            self.after_save = Some((self.intent, action));
            self.save(cx);
        } else {
            self.run_action(action, cx);
        }
    }

    fn run_action(&mut self, action: Action, cx: &mut Context<Self>) {
        self.confirm_delete = false;
        self.source_open = false;
        match action {
            Action::Section(section) => {
                self.section = section;
                if section == Section::Source {
                    self.pending_focus = Some(Field::Summary);
                }
            }
            Action::Record(id) => self.start_recording(&id, cx),
            Action::Summarize => self.summarize(cx),
        }
        cx.notify();
    }

    pub(super) fn open(&mut self, id: String, cx: &mut Context<Self>) {
        self.next_open = None;
        if self
            .document
            .as_ref()
            .is_some_and(|doc| doc.id() == id && doc.editable())
        {
            return;
        }
        self.next_open = Some(PendingOpen {
            epoch: self.open_epoch,
            intent: self.intent,
            id,
            retry: true,
        });
        self.open_next(cx);
    }

    pub(super) fn opening_document(&self, id: &str) -> bool {
        self.next_open
            .as_ref()
            .or(self.opening.as_ref())
            .is_some_and(|request| request.epoch == self.open_epoch && request.id == id)
    }

    fn open_next(&mut self, cx: &mut Context<Self>) {
        if self.opening.is_some() {
            return;
        }
        let Some(PendingOpen {
            epoch,
            intent,
            id,
            retry,
        }) = self.next_open.take()
        else {
            return;
        };
        if epoch != self.open_epoch {
            return;
        }
        let Some(meetings) = self.storage(cx) else {
            return;
        };
        let previous = self
            .document
            .as_ref()
            .filter(|doc| doc.id() == id)
            .and_then(|doc| doc.lease.clone());
        let drafts = self.drafts.clone();
        self.opening = Some(PendingOpen {
            epoch,
            intent,
            id: id.clone(),
            retry,
        });
        let opening_id = id.clone();
        let work = cx
            .background_executor()
            .spawn(async move { drafts.open(&meetings, &opening_id, previous) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.complete_open(epoch, intent, retry, result, cx);
                this.open_next(cx);
                if this.opening.is_none()
                    && let Some(doc) = &this.document
                    && doc.id() != id
                    && doc.awaiting_transcript
                {
                    // A recording finished while another selection was in
                    // flight. If that selection failed, hydrate this one.
                    this.open(doc.id().to_owned(), cx);
                }
            });
        })
        .detach();
    }

    fn complete_open(
        &mut self,
        epoch: u64,
        intent: u64,
        retry: bool,
        result: Result<Document, String>,
        cx: &mut Context<Self>,
    ) {
        self.opening = None;
        if !self.enabled || self.open_epoch != epoch {
            return;
        }
        match result {
            Ok(doc) => {
                if doc.state.phase.active()
                    && self
                        .live
                        .as_ref()
                        .is_some_and(|live| live.state.id == doc.id() && !live.state.phase.active())
                {
                    if retry {
                        self.next_open = Some(PendingOpen {
                            epoch,
                            intent,
                            id: doc.id().to_owned(),
                            retry: false,
                        });
                    } else {
                        self.fail(
                            "Recording state changed. Select the meeting to reload it.".into(),
                            cx,
                        );
                    }
                } else {
                    let mut doc = doc;
                    let mut capture_error = None;
                    if let Some(live) = self.live.as_mut().filter(|live| live.state.id == doc.id())
                    {
                        if live.state.phase.active() {
                            if live.prefix.is_none() {
                                live.keep_corrected_prefix(&doc);
                            }
                            doc.state = live.state.clone();
                        } else if live.state.run != doc.state.run {
                            capture_error = live.state.error.clone();
                        }
                    }
                    // Defaults and saved draft views apply only until a newer
                    // explicit tab choice. Keep that choice even for queued opens.
                    let view = (self.section, self.pending_focus);
                    self.install(Some(doc), cx);
                    if self.intent != intent {
                        (self.section, self.pending_focus) = view;
                    }
                    self.finish_note_selection();
                    if let Some(error) = capture_error {
                        self.fail(error, cx);
                    }
                }
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(super) fn install(&mut self, mut doc: Option<Document>, cx: &mut Context<Self>) {
        self.error = None;
        self.notice = None;
        self.source_open = false;
        self.pending_focus = None;
        let new_id = doc.as_ref().map(Document::id);
        let old_id = self.document.as_ref().map(Document::id);
        let changed_document = old_id != new_id;
        if changed_document {
            self.section = Section::Transcript;
            if let Some(old) = self
                .document
                .as_ref()
                .filter(|doc| doc.unrecorded() && !doc.has_content())
            {
                self.source_overrides.remove(old.id());
            }
        } else if let Some(next) = doc.as_mut().and_then(|doc| doc.saved.as_mut())
            && let Some(current) = self.document.as_mut().and_then(|doc| doc.saved.as_mut())
            && current.meeting.revision > next.meeting.revision
        {
            // A summary can complete while an editor reload or discard reads.
            // Preserve the newer document and the returned checkpoint token.
            std::mem::swap(&mut current.draft_version, &mut next.draft_version);
            std::mem::swap(current, next);
        }
        self.document = doc;
        self.preview = None;
        self.update_inputs(changed_document, cx);
        for input in [&self.transcript, &self.summary] {
            input.update(cx, |input, _| input.reset_editor_history());
        }
        self.update_preview(cx);
        self.sync_read_only(cx);
        if let Some(draft) = self.document.as_ref().and_then(|doc| doc.draft.as_ref()) {
            self.section = match draft.view {
                DraftView::Transcript => Section::Transcript,
                DraftView::Summary => Section::Source,
            };
            let field = if self.section == Section::Source {
                Field::Summary
            } else {
                Field::Transcript
            };
            // A legacy recording-title selection has no position in the
            // transcript or summary now that Notes owns the title field.
            let selection = if draft.title_focused {
                0..0
            } else {
                draft.start as usize..draft.end as usize
            };
            self.input(field)
                .update(cx, |input, cx| input.select_utf16(selection, cx));
            self.pending_focus = Some(field);
        }
        cx.notify();
    }

    pub(super) fn update_inputs(&self, changed_document: bool, cx: &mut Context<Self>) {
        for field in [Field::Transcript, Field::Summary] {
            let text = self.document.as_ref().map_or("", |doc| doc.text(field));
            self.input(field).update(cx, |input, cx| {
                if input.content.as_ref() != text {
                    let selection = (!changed_document).then(|| input.selection_utf16());
                    input.set_text(text.to_owned(), cx);
                    if let Some(selection) = selection {
                        input.select_utf16(selection, cx);
                    }
                }
            });
        }
    }

    pub(super) fn update_preview(&mut self, cx: &mut Context<Self>) {
        self.preview = self
            .document
            .as_ref()
            .and_then(|doc| doc.saved.as_ref())
            .and_then(|saved| saved.meeting.summary.as_ref())
            .map(|summary| cx.new(|cx| markdown::Document::meeting(&summary.text, cx)));
    }

    pub(super) fn save(&mut self, cx: &mut Context<Self>) {
        if !self.editable() || self.saving {
            return;
        }
        let Some(doc) = &self.document else { return };
        let Some(draft) = doc.draft.clone() else {
            return;
        };
        if doc.conflict() {
            self.after_save = None;
            return;
        }
        let Some(meetings) = self.storage(cx) else {
            return;
        };
        let doc = self.document.as_ref().unwrap();
        let id = doc.id().to_owned();
        let version = doc.version.clone().unwrap();
        let lease = doc.lease.clone().unwrap();
        let empty_temporary = doc.saved.is_none() && doc.unrecorded() && !doc.has_content();
        if let Err(error) = doc
            .write()
            .and_then(|write| self.drafts.stage(write, lease.clone(), empty_temporary))
        {
            self.after_save = None;
            self.fail(error, cx);
            return;
        }
        self.saving = true;
        self.error = None;
        let drafts = self.drafts.clone();
        let submitted_version = version.clone();
        let submitted_id = id.clone();
        let work = cx.background_executor().spawn(async move {
            drafts.flush(&meetings)?;
            if empty_temporary && lease.0.lock().unwrap().is_none() {
                Ok(None)
            } else {
                decode(meetings.save_draft(&submitted_id, &submitted_version)?).map(Some)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.complete_save(&id, version, draft, result, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn complete_save(
        &mut self,
        id: &str,
        version: String,
        draft: Draft,
        result: Result<Option<Meeting>, String>,
        cx: &mut Context<Self>,
    ) {
        self.saving = false;
        let after = self.after_save.take();
        if matches!(&result, Ok(Some(_))) {
            cx.emit(Event::Changed);
        }
        let Some(doc) = self.document.as_mut().filter(|doc| doc.id() == id) else {
            return;
        };
        match result {
            Ok(meeting) => {
                let refresh_preview = draft.summary.is_some();
                if let Some(meeting) = meeting {
                    doc.apply_save(meeting, version, draft);
                } else if doc.version.as_ref() == Some(&version) {
                    doc.draft = None;
                    doc.checkpoint_error = None;
                }
                self.update_inputs(false, cx);
                if refresh_preview {
                    self.update_preview(cx);
                }
                if let Some((intent, action)) = after
                    && self.pending_note.is_none()
                    && self.intent == intent
                    && self.document.as_ref().is_some_and(|doc| !doc.dirty())
                    && self.enabled
                {
                    self.run_action(action, cx);
                }
                self.resume_note_selection(cx);
            }
            Err(error) => self.fail(error, cx),
        }
        cx.notify();
    }

    pub(super) fn discard(&mut self, cx: &mut Context<Self>) {
        if !self.editable() || self.saving || !self.document.as_ref().is_some_and(Document::dirty) {
            return;
        }
        let Some(meetings) = self.storage(cx) else {
            return;
        };
        let doc = self.document.as_ref().unwrap();
        let id = doc.id().to_owned();
        let lease = doc.lease.clone();
        self.intent += 1;
        self.open_epoch += 1;
        self.discarding = true;
        self.sync_read_only(cx);
        let drafts = self.drafts.clone();
        let work = cx.background_executor().spawn(async move {
            let _writer = drafts.writer.lock().unwrap();
            // If the draft never reached storage, clearing the pending write
            // is enough. Keep the empty document and its local identity.
            if lease
                .as_ref()
                .is_some_and(|lease| lease.0.lock().unwrap().is_none())
            {
                drafts.clear(&id);
                return Ok(Document {
                    lease,
                    ..Document::live(State::dormant(id))
                });
            }
            // The latest input may have failed to checkpoint. Discard the
            // durable slot and then clear the matching pending memory.
            let current = meetings.open_document(&id)?;
            if let Some(version) = current["draft_version"].as_str() {
                meetings.discard_draft(&id, version)?;
            }
            drafts.clear(&id);
            decode(meetings.open_document(&id)?).map(|doc| Document::new(doc, lease))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.complete_discard(result, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn complete_discard(&mut self, result: Result<Document, String>, cx: &mut Context<Self>) {
        self.discarding = false;
        match result {
            Ok(doc) => self.install(Some(doc), cx),
            Err(error) => self.fail(error, cx),
        }
        self.sync_read_only(cx);
        self.resume_note_selection(cx);
        cx.notify();
    }

    pub(super) fn delete(&mut self, cx: &mut Context<Self>) {
        if !self.editable()
            || self.saving
            || self.analysis.is_some()
            || self
                .document
                .as_ref()
                .is_none_or(|doc| doc.saved.is_none() && !doc.unrecorded())
        {
            return;
        }
        if !self.confirm_delete {
            self.confirm_delete = true;
            cx.notify();
            return;
        }
        let Some(meetings) = self.storage(cx) else {
            return;
        };
        let doc = self.document.as_ref().unwrap();
        let id = doc.id().to_owned();
        let revision = doc.saved.as_ref().map_or(0, |saved| saved.meeting.revision);
        let lease = doc.lease.clone();
        self.intent += 1;
        self.open_epoch += 1;
        self.deleting = true;
        self.sync_read_only(cx);
        let drafts = self.drafts.clone();
        let deleting_id = id.clone();
        let note_store = self.note_store.clone();
        let work = cx.background_executor().spawn(async move {
            let _lease = lease;
            let _writer = drafts.writer.lock().unwrap();
            let mut committed = false;
            let result = meetings.delete_with_commit(&deleting_id, Some(revision), || {
                if let Some(store) = note_store {
                    let store = vorkspace_runtime::runtime::locked(&store)?;
                    let link = store
                        .note_recordings()?
                        .into_iter()
                        .find(|link| link.recording_id == deleting_id)
                        .ok_or("Recording is no longer attached to a note")?;
                    store.delete_note(&link.note_id)?;
                }
                committed = true;
                Ok(())
            });
            if committed {
                drafts.clear(&deleting_id);
                Ok(result.err())
            } else {
                result.map(|()| None)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.deleting = false;
                this.confirm_delete = false;
                match result {
                    Ok(cleanup_error) => {
                        this.emit_note_deleted(&id, cx);
                        this.install(None, cx);
                        if let Some(error) = cleanup_error {
                            this.notice = Some(format!(
                                "Note deleted. Recording cleanup will retry: {error}"
                            ));
                        }
                    }
                    Err(error) => this.fail(error, cx),
                }
                this.sync_read_only(cx);
                this.resume_note_selection(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
