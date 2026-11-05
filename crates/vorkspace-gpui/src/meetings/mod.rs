//! Recording documents embedded in Notes.
mod analysis;
pub(crate) mod capture;
mod document;
mod live;
mod note;
mod view;
pub(crate) use capture::stop_active;

use crate::{
    controls::page,
    input::{self, Input},
    markdown,
};
use document::{Action, Document, DraftBuffer, Field, Section};
use gpui::{AppContext, Context, Entity, EventEmitter, Focusable, Subscription, Task, Window};
use live::Live;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use vorkspace_meetings::{Meetings, recording::CaptureSource};
use vorkspace_runtime::runtime::{Database, locked};

pub(crate) struct Page {
    note_store: Option<Database>,
    note_ids: HashMap<String, String>,
    meetings: Result<Meetings, String>,
    cwd: String,
    hidden: Vec<String>,
    transcript: Entity<Input>,
    summary: Entity<Input>,
    preview: Option<Entity<markdown::Document>>,
    enabled: bool,
    enable_epoch: u64,
    active: bool,
    error: Option<String>,
    notice: Option<String>,
    document: Option<Document>,
    section: Section,
    pending_focus: Option<Field>,
    intent: u64,
    opening: bool,
    next_open: Option<(u64, String, bool)>,
    drafts: Arc<DraftBuffer>,
    draft_timer: Option<Task<()>>,
    draft_prefix: String,
    draft_sequence: u32,
    saving: bool,
    after_save: Option<(u64, Action)>,
    discarding: bool,
    confirm_delete: bool,
    deleting: bool,
    preparing: Option<String>,
    source_overrides: HashMap<String, CaptureSource>,
    source_anchor: crate::popover::AnchorSlot,
    source_open: bool,
    live: Option<Live>,
    poller: Option<Task<()>>,
    analysis: Option<analysis::Analysis>,
    _subscriptions: Vec<Subscription>,
}

pub(crate) enum Event {
    Changed,
    Attached {
        note_id: String,
        recording_id: String,
    },
    Deleted(String),
    SummaryReady(String),
}
impl EventEmitter<Event> for Page {}

impl Drop for Page {
    fn drop(&mut self) {
        self.cancel_analysis();
        if let Ok(meetings) = &self.meetings
            && let Err(error) = self.drafts.flush(meetings)
        {
            eprintln!("Could not checkpoint meeting draft on close: {error}");
        }
    }
}

impl Page {
    pub(crate) fn new(store: Database, cx: &mut Context<Self>) -> Self {
        let meetings = locked(&store)
            .ok()
            .and_then(|store| store.data_dir().map(PathBuf::from))
            .ok_or_else(|| "Meeting storage is unavailable.".to_owned())
            .and_then(|root| Meetings::open(&root));
        let mut page = Self::with_meetings(meetings, cx);
        page.note_store = Some(store);
        page
    }

    fn with_meetings(meetings: Result<Meetings, String>, cx: &mut Context<Self>) -> Self {
        let transcript = Self::editor(
            "Meeting transcript",
            "transcript.txt",
            vorkspace_meetings::recording::MAX_TRANSCRIPT,
            cx,
        );
        let summary = Self::editor("Summary Markdown", "summary.md", 64 * 1024, cx);
        let subscriptions = vec![
            cx.subscribe(&transcript, |this, _, _: &input::Changed, cx| {
                this.edit(Field::Transcript, cx)
            }),
            cx.subscribe(&summary, |this, _, _: &input::Changed, cx| {
                this.edit(Field::Summary, cx)
            }),
            cx.subscribe(&transcript, |this, _, event: &input::Rejected, cx| {
                this.fail(event.0.into(), cx)
            }),
            cx.subscribe(&summary, |this, _, event: &input::Rejected, cx| {
                this.fail(event.0.into(), cx)
            }),
            cx.on_app_quit(|this, cx| {
                let meetings = this.meetings.clone();
                let drafts = this.drafts.clone();
                cx.background_executor().spawn(async move {
                    if let Ok(meetings) = meetings
                        && let Err(error) = drafts.flush(&meetings)
                    {
                        eprintln!("Could not checkpoint meeting draft on quit: {error}");
                    }
                })
            }),
        ];
        Self {
            note_store: None,
            note_ids: HashMap::new(),
            meetings,
            cwd: String::new(),
            hidden: Vec::new(),
            transcript,
            summary,
            preview: None,
            enabled: true,
            enable_epoch: 0,
            active: false,
            error: None,
            notice: None,
            document: None,
            section: Section::Transcript,
            pending_focus: None,
            intent: 0,
            opening: false,
            next_open: None,
            drafts: Arc::default(),
            draft_timer: None,
            draft_prefix: uuid::Uuid::new_v4().simple().to_string()[..24].to_owned(),
            draft_sequence: 0,
            saving: false,
            after_save: None,
            discarding: false,
            confirm_delete: false,
            deleting: false,
            preparing: None,
            source_overrides: HashMap::new(),
            source_anchor: crate::popover::AnchorSlot::default(),
            source_open: false,
            live: None,
            poller: None,
            analysis: None,
            _subscriptions: subscriptions,
        }
    }

    fn editor(
        label: &'static str,
        path: &str,
        max_bytes: usize,
        cx: &mut Context<Self>,
    ) -> Entity<Input> {
        cx.new(|cx| {
            let mut input = Input::document_editor(label, path, max_bytes, cx);
            input.set_font_size(page::BODY_SIZE);
            input.set_line_height(page::BODY_LINE);
            input
        })
    }

    fn selected_source(&self, doc: &Document) -> CaptureSource {
        self.source_overrides
            .get(doc.id())
            .copied()
            .unwrap_or(doc.state.source)
    }

    pub(crate) fn configure_context(&mut self, cwd: &str, hidden: &[String]) {
        self.cwd = cwd.into();
        self.hidden = hidden.into();
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            cx.emit(Event::Changed);
            self.start_polling(cx);
        } else {
            self.poller = None;
            self.source_open = false;
        }
        cx.notify();
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.enable_epoch += 1;
            if !enabled {
                self.intent += 1;
                self.set_active(false, cx);
                self.cancel_analysis();
            }
            self.sync_read_only(cx);
            cx.notify();
        }
    }

    fn storage(&mut self, cx: &mut Context<Self>) -> Option<Meetings> {
        if !self.enabled {
            return None;
        }
        match self.meetings.clone() {
            Ok(meetings) => Some(meetings),
            Err(error) => {
                self.fail(error, cx);
                None
            }
        }
    }

    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }

    fn input(&self, field: Field) -> &Entity<Input> {
        match field {
            Field::Transcript => &self.transcript,
            Field::Summary => &self.summary,
        }
    }

    /// Notes calls this only while this recording's editor is visible. The
    /// focus transfer happens at that owning render boundary, without a delayed
    /// callback that could reclaim focus after Notes changes its selection.
    pub(crate) fn focus_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.active || !self.enabled {
            return;
        }
        if self
            .document
            .as_ref()
            .is_some_and(|doc| doc.state.phase.active() || doc.awaiting_transcript)
        {
            return;
        }
        let Some(field) = self.pending_focus.take() else {
            return;
        };
        if matches!(
            (field, self.section),
            (Field::Transcript, Section::Transcript) | (Field::Summary, Section::Source)
        ) {
            window.focus(&self.input(field).focus_handle(cx), cx);
        }
    }

    fn editable(&self) -> bool {
        self.enabled
            && !self.deleting
            && !self.discarding
            && self
                .document
                .as_ref()
                .is_some_and(|doc| self.preparing.as_deref() != Some(doc.id()) && doc.editable())
    }

    fn sync_read_only(&self, cx: &mut Context<Self>) {
        let read_only = !self.editable();
        for input in [&self.transcript, &self.summary] {
            input.update(cx, |input, cx| {
                input.set_read_only(read_only);
                cx.notify();
            });
        }
    }
}
