//! The desktop Notes split view, backed by the same notes and image assets.
use vorkspace_ui::radius;
mod recording;
use crate::{
    controls::page,
    file_tools::explorer::DraggedFile,
    input::{self, Input},
    markdown::Document,
    popover::{Anchor, AnchorSlot, popover},
};
use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, ExternalPaths, FocusHandle,
    Focusable, FontWeight, MouseButton, Role, ScrollHandle, Subscription, Task, Window, div,
    prelude::*, px,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};
use vorkspace_core::notes::{self, Note};
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::theme::*;

pub(crate) enum Event {
    Close,
    OpenTab(String),
    OpenSource(i64),
    AddToChat(Note),
}

pub(crate) struct Notes {
    store: Database,
    notes: Vec<Note>,
    /// A document tab keeps its note identity even if the note is deleted.
    /// The library is the only view allowed to change selection.
    pinned: Option<String>,
    /// Open document tabs remain in the library list, but selecting one
    /// activates its existing editor instead of opening a second editor.
    open_tabs: HashSet<String>,
    selected: Option<String>,
    saved_note: Option<Note>,
    save_conflict: bool,
    query: Entity<Input>,
    focus: FocusHandle,
    title: Entity<Input>,
    body: Entity<Input>,
    tag: Entity<Input>,
    project_query: Entity<Input>,
    project_anchor: AnchorSlot,
    project_open: bool,
    cwd: String,
    projects: Vec<String>,
    appearances: HashMap<String, vorkspace_core::project_appearance::ProjectAppearance>,
    source: bool,
    modes: HashMap<String, bool>,
    document: Option<Entity<Document>>,
    recording: Entity<crate::meetings::Page>,
    recordings: HashMap<String, String>,
    show_recording: bool,
    ephemeral: HashSet<String>,
    confirm_delete: bool,
    dirty: bool,
    revision: u64,
    load_epoch: u64,
    loading: bool,
    error: Option<String>,
    debounce: Option<Task<()>>,
    image_note: Option<String>,
    image_drag: bool,
    list_width: f32,
    pane_width: Option<f32>,
    list_scroll: ScrollHandle,
    detail_scroll: ScrollHandle,
    show_list: bool,
    resize: Option<(f32, f32)>,
    blurs_bound: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Event> for Notes {}
impl Focusable for Notes {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Notes {
    pub(crate) fn new(store: Database, cx: &mut Context<Self>) -> Self {
        Self::with_pinned(store, None, cx)
    }

    pub(crate) fn open_note_tab(store: Database, note_id: String, cx: &mut Context<Self>) -> Self {
        Self::with_pinned(store, Some(note_id), cx)
    }

    fn with_pinned(store: Database, pinned: Option<String>, cx: &mut Context<Self>) -> Self {
        let recording = cx.new(|cx| crate::meetings::Page::new(store.clone(), cx));
        let query =
            cx.new(|cx| Input::field("Filter notes", "Filter notes", page::secondary_ink(), cx));
        let title = cx.new(|cx| {
            let mut input = Input::field("Note title", "Untitled", page::secondary_ink(), cx);
            input.set_font_size(page::TITLE_SIZE);
            input.set_line_height(page::TITLE_LINE);
            input.set_max_chars(notes::MAX_TITLE);
            input
        });
        let body = cx.new(|cx| {
            let mut input = Input::document_editor("Note Markdown", "note.md", notes::MAX_BODY, cx);
            input.set_font_size(page::BODY_SIZE);
            input.set_line_height(page::BODY_LINE);
            input
        });
        let tag = cx.new(|cx| {
            let mut input = Input::field("Add note tag", "Add tag…", page::secondary_ink(), cx);
            input.set_font_size(page::META_SIZE);
            input.set_max_chars(notes::MAX_TAG_LENGTH + 1);
            input
        });
        let project_query =
            cx.new(|cx| Input::field("Find project", "Find project…", page::secondary_ink(), cx));
        let subscriptions = vec![
            cx.on_app_quit(|this, cx| {
                this.flush(cx);
                std::future::ready(())
            }),
            cx.observe(&recording, |_, _, cx| cx.notify()),
            cx.subscribe(&recording, |this, _, event: &crate::meetings::Event, cx| {
                match event {
                    crate::meetings::Event::Attached {
                        note_id,
                        recording_id,
                    } => {
                        this.revision = this.revision.wrapping_add(1);
                        this.recordings
                            .insert(note_id.clone(), recording_id.clone());
                        if this.selected.as_ref() == Some(note_id) {
                            this.show_recording = true;
                        }
                    }
                    crate::meetings::Event::Deleted(id) => this.forget_note(id, cx),
                    crate::meetings::Event::Changed => this.refresh(cx),
                    crate::meetings::Event::SummaryReady(id) => {
                        if this
                            .selected
                            .as_ref()
                            .and_then(|note| this.recordings.get(note))
                            == Some(id)
                        {
                            this.show_recording = true;
                        }
                    }
                }
                cx.notify();
            }),
            cx.subscribe(&query, |_, _, _: &input::Changed, cx| cx.notify()),
            cx.subscribe(&title, |this, _, _: &input::Changed, cx| {
                this.input_changed(cx)
            }),
            cx.subscribe(&body, |this, _, _: &input::Changed, cx| {
                this.highlight_source(cx);
                this.input_changed(cx);
            }),
            cx.subscribe(&body, |this, _, event: &input::Rejected, cx| {
                this.error = Some(event.0.into());
                cx.notify();
            }),
            cx.subscribe(&tag, |this, _, _: &input::Changed, cx| {
                if this.tag.read(cx).content.ends_with(',') {
                    this.add_tag(cx);
                }
            }),
            cx.subscribe(&tag, |this, _, _: &input::Submitted, cx| this.add_tag(cx)),
            cx.subscribe(&title, |this, _, _: &input::Submitted, cx| this.flush(cx)),
            cx.subscribe(&project_query, |_, _, _: &input::Changed, cx| cx.notify()),
        ];
        let mut this = Self {
            store,
            notes: Vec::new(),
            selected: pinned.clone(),
            saved_note: None,
            save_conflict: false,
            pinned,
            open_tabs: HashSet::new(),
            query,
            focus: cx.focus_handle(),
            title,
            body,
            tag,
            project_query,
            project_anchor: AnchorSlot::default(),
            project_open: false,
            cwd: String::new(),
            projects: Vec::new(),
            appearances: HashMap::new(),
            source: false,
            modes: HashMap::new(),
            document: None,
            recording,
            recordings: HashMap::new(),
            show_recording: false,
            ephemeral: HashSet::new(),
            confirm_delete: false,
            dirty: false,
            revision: 0,
            load_epoch: 0,
            loading: true,
            error: None,
            debounce: None,
            image_note: None,
            image_drag: false,
            list_width: page::LIST_WIDTH,
            pane_width: None,
            list_scroll: ScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            show_list: false,
            resize: None,
            blurs_bound: false,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    pub(crate) fn pane_note_id(&self) -> Option<String> {
        self.pinned.clone()
    }

    pub(crate) fn set_pane_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if width.is_finite() && width > 0. && self.pane_width != Some(width) {
            self.pane_width = Some(width);
            cx.notify();
        }
    }

    fn available_width(&self, window: &Window) -> f32 {
        self.pane_width
            .unwrap_or_else(|| window.viewport_size().width.as_f32())
    }

    fn is_narrow(&self, window: &Window) -> bool {
        self.pane_width
            .map_or_else(|| page::narrow(window), |width| width < 900.)
    }

    pub(crate) fn selected_note_id(&self) -> Option<String> {
        self.selected.clone()
    }

    /// A native note write must not supersede this editor's local intent.
    /// Read the inputs as well as `dirty`: their Changed event may still be
    /// queued when an external write reaches Shell.
    pub(crate) fn blocks_external_write(&self, note_id: &str, cx: &App) -> bool {
        if self.selected.as_deref() != Some(note_id) {
            return false;
        }
        if self.dirty
            || self.ephemeral.contains(note_id)
            || self.image_note.as_deref() == Some(note_id)
            || !self.tag.read(cx).content.trim().is_empty()
        {
            return true;
        }
        let inputs_changed = self.draft(cx).is_some_and(|draft| {
            self.notes
                .iter()
                .find(|note| note.id == note_id)
                .is_none_or(|saved| {
                    draft.title != saved.title
                        || draft.body != saved.body
                        || draft.tags != saved.tags
                        || draft.source_cwd != saved.source_cwd
                })
        });
        inputs_changed || self.recording.read(cx).has_pending_note_edit(note_id)
    }

    pub(crate) fn set_open_note_tabs(&mut self, ids: HashSet<String>, cx: &mut Context<Self>) {
        if self.pinned.is_some() {
            return;
        }
        let changed = self.open_tabs != ids;
        self.open_tabs = ids;
        if self
            .selected
            .as_ref()
            .is_some_and(|id| self.open_tabs.contains(id))
        {
            self.flush(cx);
            if !self.dirty {
                let next = self
                    .notes
                    .iter()
                    .find(|note| !self.open_tabs.contains(&note.id))
                    .map(|note| note.id.clone());
                self.load_selection(next, cx);
            }
        } else if changed {
            cx.notify();
        }
    }

    /// Hand the library's selected editor to a document tab without losing
    /// its inputs or the recording edit lease. Shell can replace the library
    /// entity after this succeeds.
    pub(crate) fn pin_selected_note(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if self.pinned.is_some()
            || self.selected.as_deref() != Some(id)
            || self.ephemeral.contains(id)
            || !self.notes.iter().any(|note| note.id == id)
        {
            return false;
        }
        self.pinned = Some(id.to_owned());
        self.show_list = false;
        cx.notify();
        true
    }

    pub(crate) fn pane_title(&self, cx: &App) -> String {
        let Some(id) = self.pinned.as_deref() else {
            return "Notes".into();
        };
        if !self.notes.iter().any(|note| note.id == id) {
            return "Notes".into();
        }
        let title = self.title.read(cx).content.trim();
        if title.is_empty() {
            "Untitled".into()
        } else {
            title.into()
        }
    }

    pub(crate) fn set_appearances(
        &mut self,
        appearances: HashMap<String, vorkspace_core::project_appearance::ProjectAppearance>,
        cx: &mut Context<Self>,
    ) {
        self.appearances = appearances;
        cx.notify();
    }

    fn project_mark(&self, path: &str, side: f32) -> AnyElement {
        crate::project_settings::project_icon(
            &self.appearances.get(path).cloned().unwrap_or_default(),
            &project_name(path),
            side,
        )
    }

    fn highlight_source(&self, cx: &mut Context<Self>) {
        let text = self.body.read(cx).content.clone();
        let mut at = 0;
        let mut analysis = vorkspace_editor::editor_analysis::Analysis::default();
        for line in text.split_inclusive('\n') {
            let start = at;
            at += line.len();
            let trimmed = line.trim_start_matches(' ');
            let count = trimmed.bytes().take_while(|byte| *byte == b'#').count();
            if line.len() - trimmed.len() <= 3
                && (1..=6).contains(&count)
                && trimmed[count..].starts_with(char::is_whitespace)
            {
                analysis
                    .tokens
                    .push(vorkspace_editor::editor_analysis::Token {
                        from: start,
                        to: at - usize::from(line.ends_with('\n')),
                        kind: "var(--color-markdown-heading)".into(),
                    });
            }
        }
        self.body
            .update(cx, |input, cx| input.set_editor_analysis(analysis, cx));
    }

    pub(crate) fn set_context(
        &mut self,
        cwd: String,
        projects: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.cwd = cwd;
        self.projects = projects;
        cx.notify();
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.image_note.is_some() {
            return;
        }
        self.flush(cx);
        if self.dirty {
            return;
        }
        let revision = self.revision;
        self.load_epoch = self.load_epoch.wrapping_add(1);
        let epoch = self.load_epoch;
        let store = self.store.clone();
        let pinned = self.pinned.clone();
        let work = cx.background_executor().spawn(async move {
            if let Some(id) = pinned {
                recording::load_pinned(&store, &id)
            } else {
                recording::load(&store)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                if this.revision != revision || this.load_epoch != epoch {
                    return;
                }
                match result {
                    Ok((mut notes, recordings)) => {
                        this.recordings = recordings;
                        this.ephemeral
                            .retain(|id| !notes.iter().any(|note| &note.id == id));
                        notes.extend(
                            this.notes
                                .iter()
                                .filter(|note| this.ephemeral.contains(&note.id))
                                .cloned(),
                        );
                        // Preserve edits made while the refresh was in flight.
                        if this.dirty
                            && let Some(draft) = this.draft(cx)
                        {
                            notes.retain(|note| note.id != draft.id);
                            notes.insert(0, draft);
                        }
                        let content_changed = this.selected.as_ref().is_some_and(|id| {
                            let before = this.notes.iter().find(|note| &note.id == id);
                            let after = notes.iter().find(|note| &note.id == id);
                            match (before, after) {
                                (Some(a), Some(b)) => {
                                    a.title != b.title
                                        || a.body != b.body
                                        || a.source_cwd != b.source_cwd
                                }
                                _ => true,
                            }
                        });
                        this.notes = notes;
                        let next = this.pinned.clone().or_else(|| {
                            this.selected
                                .clone()
                                .filter(|id| {
                                    !this.open_tabs.contains(id)
                                        && this.notes.iter().any(|note| &note.id == id)
                                })
                                .or_else(|| {
                                    this.notes
                                        .iter()
                                        .find(|note| !this.open_tabs.contains(&note.id))
                                        .map(|note| note.id.clone())
                                })
                        });
                        if next != this.selected
                            || this.document.is_none()
                            || (!this.dirty && content_changed)
                        {
                            this.load_selection(next, cx);
                        } else if let Some(id) = &this.selected {
                            if !this.dirty && !this.ephemeral.contains(id) {
                                this.saved_note =
                                    this.notes.iter().find(|note| &note.id == id).cloned();
                            }
                            let recording_id = this.recordings.get(id);
                            this.recording.update(cx, |page, cx| {
                                page.show_note(id, recording_id.map(String::as_str), cx)
                            });
                        }
                        this.error = None;
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn draft(&self, cx: &App) -> Option<Note> {
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
    fn input_changed(&mut self, cx: &mut Context<Self>) {
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

    fn edited(&mut self, cx: &mut Context<Self>) {
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

    fn persist(&mut self, finalize: bool, cx: &mut Context<Self>) {
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

    fn resolve_save_conflict(&mut self, save_copy: bool, cx: &mut Context<Self>) {
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
                if let Some(copy) = copy {
                    let id = copy.id.clone();
                    self.notes.insert(0, copy);
                    cx.emit(Event::OpenTab(id));
                }
            }
            Err(error) => self.error = Some(format!("Could not resolve the note edit: {error}")),
        }
        cx.notify();
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        if self.pinned.as_ref().is_some_and(|pinned| pinned != &id) {
            return;
        }
        if self.pinned.is_none() && self.open_tabs.contains(&id) {
            self.request_open_tab(&id, cx);
            return;
        }
        if self.selected.as_ref() == Some(&id) {
            self.show_list = false;
            cx.notify();
            return;
        }
        self.flush(cx);
        if self.dirty {
            return;
        }
        self.discard_empty_draft(cx);
        self.load_selection(Some(id), cx);
    }

    fn load_selection(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.pinned.is_some() && id.as_ref() != self.pinned.as_ref() {
            return;
        }
        let changed = self.selected != id;
        if changed {
            self.confirm_delete = false;
            self.project_open = false;
            self.show_list = false;
            self.detail_scroll = ScrollHandle::new();
        }
        self.selected = id;
        self.dirty = false;
        self.save_conflict = false;
        let note = self
            .notes
            .iter()
            .find(|note| Some(&note.id) == self.selected.as_ref())
            .cloned();
        self.saved_note = note
            .as_ref()
            .filter(|note| !self.ephemeral.contains(&note.id))
            .cloned();
        if let Some(note) = &note {
            let recording_id = self.recordings.get(&note.id).cloned();
            if changed {
                self.show_recording = recording_id.is_some();
            }
            self.recording.update(cx, |page, cx| {
                page.show_note(&note.id, recording_id.as_deref(), cx)
            });
        } else {
            self.show_recording = false;
            self.recording.update(cx, |page, cx| page.clear_note(cx));
        }
        self.title.update(cx, |input, cx| {
            input.set_text(
                note.as_ref()
                    .map(|note| note.title.clone())
                    .unwrap_or_default(),
                cx,
            )
        });
        self.body.update(cx, |input, cx| {
            input.set_text(
                note.as_ref()
                    .map(|note| note.body.clone())
                    .unwrap_or_default(),
                cx,
            )
        });
        self.highlight_source(cx);
        self.body
            .update(cx, |input, cx| input.set_word_wrap(true, cx));
        self.tag
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.source = note.as_ref().is_some_and(|note| {
            self.modes.get(&note.id).copied().unwrap_or(
                note.body.trim().is_empty() && (note.title.is_empty() || note.title == "Untitled"),
            )
        });
        self.update_preview(cx);
        cx.notify();
    }

    fn update_preview(&mut self, cx: &mut Context<Self>) {
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

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pinned.is_some() {
            return;
        }
        self.flush(cx);
        if self.dirty {
            return;
        }
        self.discard_empty_draft(cx);
        let now = vorkspace_core::time::now_ms().min(i64::MAX as u64) as i64;
        let note = Note {
            id: format!("note-{}", uuid::Uuid::new_v4().simple()),
            slug: String::new(),
            title: String::new(),
            body: String::new(),
            tags: Vec::new(),
            source_session_id: None,
            source_cwd: notes::is_project(&self.cwd).then(|| self.cwd.clone()),
            slug_pending: true,
            created_at: now,
            updated_at: now,
        };
        self.ephemeral.insert(note.id.clone());
        self.revision = self.revision.wrapping_add(1);
        let id = note.id.clone();
        self.notes.insert(0, note);
        self.query
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.load_selection(Some(id), cx);
        window.focus(&self.body.focus_handle(cx), cx);
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.image_note.as_ref() == Some(&id) {
            return;
        }
        if !self.confirm_delete {
            self.confirm_delete = true;
            cx.notify();
            return;
        }
        if self.recordings.contains_key(&id) {
            self.recording
                .update(cx, |page, cx| page.delete_note_recording(cx));
        } else {
            self.delete_saved_note(&id, cx);
        }
    }

    fn delete_saved_note(&mut self, id: &str, cx: &mut Context<Self>) {
        let selected = self.selected.as_deref() == Some(id);
        if selected {
            self.debounce = None;
        }
        let result = locked(&self.store).and_then(|store| store.delete_note(id));
        match result {
            Ok(()) => self.forget_note(id, cx),
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn forget_note(&mut self, id: &str, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        self.notes.retain(|note| note.id != id);
        self.modes.remove(id);
        self.ephemeral.remove(id);
        self.recordings.remove(id);
        if self.selected.as_deref() == Some(id) {
            self.debounce = None;
            self.dirty = false;
            self.load_selection(
                self.pinned.clone().or_else(|| {
                    self.notes
                        .iter()
                        .find(|note| !self.open_tabs.contains(&note.id))
                        .map(|note| note.id.clone())
                }),
                cx,
            );
        }
        cx.notify();
    }

    fn request_open_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.notes.iter().any(|note| note.id == id) {
            return;
        }
        self.flush(cx);
        if self.dirty || self.ephemeral.contains(id) {
            return;
        }
        cx.emit(Event::OpenTab(id.to_owned()));
    }

    fn add_tag(&mut self, cx: &mut Context<Self>) {
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

    fn remove_tag(&mut self, tag: &str, cx: &mut Context<Self>) {
        if let Some(note) = self
            .notes
            .iter_mut()
            .find(|note| Some(&note.id) == self.selected.as_ref())
        {
            note.tags.retain(|saved| saved != tag);
            self.edited(cx);
        }
    }

    fn move_project(&mut self, path: String, cx: &mut Context<Self>) {
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

    fn add_images(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
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

    fn project_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.project_query.read(cx).content.to_lowercase();
        let mut projects = self.projects.clone();
        if notes::is_project(&self.cwd) {
            projects.insert(0, self.cwd.clone());
        }
        for note in &self.notes {
            if let Some(path) = note
                .source_cwd
                .as_ref()
                .filter(|path| notes::is_project(path))
            {
                projects.push(path.clone());
            }
        }
        let mut seen = std::collections::HashSet::new();
        projects.retain(|path| seen.insert(path.clone()) && path.to_lowercase().contains(&query));
        let weak = cx.weak_entity();
        popover(
            "note-project-picker",
            Anchor::Slot(self.project_anchor.clone()),
        )
        .width(300.)
        .max_height(340.)
        .on_dismiss(move |_, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.project_open = false;
                cx.notify();
            });
        })
        .child(
            div()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().px_2().py_1().child(self.project_query.clone()))
                .child(
                    div()
                        .id("note-no-project")
                        .role(Role::Button)
                        .map(vorkspace_ui::theme::tab_reachable)
                        .aria_label("No project")
                        .px_2()
                        .py_1p5()
                        .rounded(px(radius::CONTAINER))
                        .text_size(px(12.))
                        .hover(|style| style.bg(vorkspace_ui::fluid::hover(0.08)))
                        .child("No project")
                        .on_click(cx.listener(|this, _, _, cx| this.move_project("~".into(), cx))),
                )
                .child(
                    div()
                        .id("note-project-results")
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(projects.into_iter().enumerate().map(|(ix, path)| {
                            let name = project_name(&path);
                            div()
                                .id(("note-project", ix))
                                .role(Role::Button)
                                .aria_label(path.clone())
                                .px_2()
                                .py_1p5()
                                .rounded(px(radius::CONTAINER))
                                .text_size(px(12.))
                                .hover(|style| style.bg(vorkspace_ui::fluid::hover(0.08)))
                                .child(name)
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(page::META_SIZE))
                                        .text_color(page::secondary_ink())
                                        .child(path.clone()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_project(path.clone(), cx)
                                }))
                        })),
                ),
        )
        .into_any_element()
    }

    fn shareable_note(&self, cx: &App) -> Option<Note> {
        let mut note = self.draft(cx)?;
        if self.show_recording {
            let recording = self.recording.read(cx);
            if !recording.is_for_note(&note.id) {
                return None;
            }
            note.body = recording.selected_text()?.to_owned();
        }
        (!note.body.trim().is_empty()).then_some(note)
    }

    fn detail(&mut self, note: Note, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let source = note.source_session_id.clone();
        let attached = self.recordings.contains_key(&note.id);
        let recording_ready = self.recording.read(cx).is_for_note(&note.id);
        let can_add = if self.show_recording {
            recording_ready
                && self
                    .recording
                    .read(cx)
                    .selected_text()
                    .is_some_and(|text| !text.trim().is_empty())
        } else {
            !note.body.trim().is_empty()
        };
        let can_delete = self.image_note.as_ref() != Some(&note.id)
            && (!attached || (recording_ready && self.recording.read(cx).can_delete_note()));
        let open_tab = self.pinned.is_none();
        let tab_note_id = note.id.clone();
        let project = page::action("note-move-project", "Move note to project", true)
            .relative()
            .min_w_0()
            .max_w_full()
            .child(self.project_anchor.probe())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        note.source_cwd
                            .as_deref()
                            .map(|path| self.project_mark(path, page::ICON_SIZE))
                            .unwrap_or_else(|| icon("folder", page::ICON_SIZE).into_any_element()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(note.project_name().unwrap_or_else(|| "No project".into())),
                    )
                    .child(icon("chevron-down", page::ICON_SIZE)),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.project_open = !this.project_open;
                if this.project_open {
                    this.project_query
                        .update(cx, |input, cx| input.set_text(String::new(), cx));
                    window.focus(&this.project_query.focus_handle(cx), cx);
                }
                cx.notify();
            }));
        let tags = page::meta()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child("Tags")
            .children(note.tags.iter().enumerate().map(|(ix, tag)| {
                let tag = tag.clone();
                div()
                    .h(px(page::CONTROL_HEIGHT))
                    .max_w(px(192.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded(px(radius::CONTROL))
                    .bg(content(0.08))
                    .pl_2()
                    .pr_1()
                    .text_color(content(0.7))
                    .child(div().truncate().child(format!("#{tag}")))
                    .child(
                        page::icon_action(("remove-note-tag", ix), format!("Remove #{tag}"), true)
                            .child(icon("close", page::ICON_SIZE))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_tag(&tag, cx))),
                    )
            }))
            .when(note.tags.len() < notes::MAX_TAGS, |d| {
                d.child(page::field_frame().w(px(180.)).child(self.tag.clone()))
            });
        let actions = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .child(
                page::primary("note-add-to-chat", "Add to chat", !can_add)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if can_add {
                            this.flush(cx);
                            if !this.dirty
                                && let Some(note) = this.shareable_note(cx)
                            {
                                cx.emit(Event::AddToChat(note));
                            }
                        }
                    }))
                    .child("Add to chat"),
            )
            .when_some(source, |d, source| {
                d.child(
                    page::action("note-source-session", "Open source conversation", true)
                        .child("Source conversation")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.flush(cx);
                            if this.dirty {
                                return;
                            }
                            if let Some(id) = locked(&this.store)
                                .ok()
                                .and_then(|store| store.resolve_existing_session_id(&source))
                            {
                                cx.emit(Event::OpenSource(id));
                            } else {
                                this.error = Some("Source conversation no longer exists.".into());
                                cx.notify();
                            }
                        })),
                )
            })
            .child(
                page::icon_action("copy-note", "Copy selected note text", can_add)
                    .child(icon("response-copy", page::ICON_SIZE))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(note) = this.shareable_note(cx) {
                            cx.write_to_clipboard(ClipboardItem::new_string(note.body));
                        }
                    })),
            )
            // Deleting is rare and destructive, so it waits at the far end as
            // a quiet icon and only grows into words once it asks to confirm.
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(self.confirm_delete, |d| {
                        d.child(
                            page::action("cancel-delete-note", "Cancel deletion", true)
                                .child("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirm_delete = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            page::secondary("delete-note", "Delete note", true, !can_delete)
                                .child(icon("trash", page::ICON_SIZE))
                                .child("Confirm delete")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if can_delete {
                                        this.delete(cx);
                                    }
                                })),
                        )
                    })
                    .when(!self.confirm_delete, |d| {
                        d.child(
                            page::icon_action("delete-note", "Delete note", can_delete)
                                .child(icon("trash", page::ICON_SIZE))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if can_delete {
                                        this.delete(cx);
                                    }
                                })),
                        )
                    }),
            );
        let actions = if recording_ready {
            self.recording.update(cx, |page, cx| {
                page.note_controls(actions.into_any_element(), self.show_recording, cx)
            })
        } else {
            actions.into_any_element()
        };
        let tabs = div()
            .id("note-sections")
            .role(Role::TabList)
            .aria_label("Note sections")
            .h(px(page::TOOLBAR_HEIGHT))
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(page::divider())
            .children(
                [(false, "Preview"), (true, "Source")].map(|(source, label)| {
                    page::tab(
                        if source {
                            "note-source"
                        } else {
                            "note-preview"
                        },
                        label,
                        !self.show_recording && self.source == source,
                    )
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.source = source;
                        this.show_recording = false;
                        this.recording
                            .update(cx, |page, cx| page.hide_note_content(cx));
                        if let Some(id) = &this.selected {
                            this.modes.insert(id.clone(), source);
                        }
                        if source {
                            window.focus(&this.body.focus_handle(cx), cx);
                        } else {
                            this.flush(cx);
                            this.update_preview(cx);
                        }
                        cx.notify();
                    }))
                }),
            )
            .when(attached, |d| {
                d.children(
                    [(false, "Summary"), (true, "Transcript")].map(|(transcript, label)| {
                        page::tab(
                            if transcript {
                                "note-transcript"
                            } else {
                                "note-summary"
                            },
                            label,
                            self.show_recording
                                && self.recording.read(cx).transcript_selected() == transcript,
                        )
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.flush(cx);
                            if this.dirty {
                                return;
                            }
                            this.show_recording = true;
                            this.recording
                                .update(cx, |page, cx| page.note_section(transcript, cx));
                            cx.notify();
                        }))
                    }),
                )
            });
        let editor = div()
            .id("note-image-drop-zone")
            .relative()
            .min_h(px(448.))
            .rounded(px(radius::CONTAINER))
            .border_1()
            .border_color(if self.image_drag {
                accent_color()
            } else {
                content(0.)
            })
            .on_drag_move::<ExternalPaths>(cx.listener(
                |this, event: &gpui::DragMoveEvent<ExternalPaths>, _, cx| {
                    this.image_drag = event.bounds.contains(&event.event.position);
                    cx.notify();
                },
            ))
            .on_drag_move::<DraggedFile>(cx.listener(
                |this, event: &gpui::DragMoveEvent<DraggedFile>, _, cx| {
                    this.image_drag = event.bounds.contains(&event.event.position);
                    cx.notify();
                },
            ))
            .on_file_drop_exit(cx.listener(|this, _, _, cx| {
                this.image_drag = false;
                cx.notify();
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_images(paths.paths().to_vec(), cx)
            }))
            .on_drop(cx.listener(|this, file: &DraggedFile, _, cx| {
                this.add_images(vec![file.0.clone()], cx)
            }))
            .when(self.source, |d| {
                d.h(px((self.body.read(cx).editor_line_count() as f32
                    * page::BODY_LINE
                    + 16.)
                    .clamp(448., 2000.)))
                    .child(self.body.clone())
            })
            .when(!self.source, |d| {
                d.when_some(self.document.clone(), |d, preview| d.child(preview))
            })
            .when(self.image_drag || self.image_note.is_some(), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(radius::CONTAINER))
                        .bg(pane_fill().opacity(0.9))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(content(0.7))
                        .child(if self.image_note.is_some() {
                            "Adding images…"
                        } else {
                            "Drop images here"
                        }),
                )
            });
        div()
            .id("note-detail")
            .debug_selector(|| "note-detail".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.detail_scroll)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(16.))
                    .w_full()
                    .max_w(px(page::DETAIL_MAX))
                    .mx_auto()
                    .p(page::detail_padding(narrow))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                page::meta()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(icon("note", page::ICON_SIZE))
                                    .child("Note")
                                    .child(div().min_w_0().truncate().child(note.slug.clone()))
                                    .child(project)
                                    .when(open_tab, |d| {
                                        d.child(
                                            page::icon_action(
                                                "open-note-tab",
                                                "Open note in tab",
                                                true,
                                            )
                                            .debug_selector(|| "open-note-tab".into())
                                            .child(icon("external-link", page::ICON_SIZE))
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    this.request_open_tab(&tab_note_id, cx)
                                                }),
                                            ),
                                        )
                                    }),
                            )
                            .child(page::title().child(self.title.clone()))
                            .child(
                                page::meta()
                                    .child(format!("Updated {}", relative_time(note.updated_at))),
                            )
                            .child(tags)
                            .child(actions),
                    )
                    .child(tabs)
                    .child(if self.show_recording && recording_ready {
                        self.recording.read(cx).note_content()
                    } else {
                        editor.into_any_element()
                    }),
            )
            .into_any_element()
    }
}

impl gpui::Render for Notes {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.blurs_bound {
            self.blurs_bound = true;
            self._subscriptions.push(cx.on_blur(
                &self.title.focus_handle(cx),
                window,
                |this, _, cx| this.flush(cx),
            ));
            self._subscriptions.push(cx.on_blur(
                &self.tag.focus_handle(cx),
                window,
                |this, _, cx| this.add_tag(cx),
            ));
        }
        if !cx.has_active_drag() {
            self.image_drag = false;
        }
        let query = self.query.read(cx).content.to_string();
        let visible: Vec<_> = self
            .notes
            .iter()
            .filter(|note| note.matches(&query))
            .cloned()
            .collect();
        let narrow = self.is_narrow(window);
        let show_list =
            self.pinned.is_none() && (!narrow || self.show_list || self.selected.is_none());
        let show_detail = self.pinned.is_some() || !narrow || !show_list;
        if show_detail
            && self.show_recording
            && self
                .selected
                .as_deref()
                .is_some_and(|id| self.recording.read(cx).is_for_note(id))
        {
            self.recording
                .update(cx, |page, cx| page.focus_pending(window, cx));
        }
        let list = div()
            .relative()
            .w(px(self.list_width))
            .when(narrow, |d| d.w_full())
            .h_full()
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                page::toolbar()
                    .child(
                        page::search_frame(&self.query.focus_handle(cx))
                            .flex_1()
                            .child(icon("search", page::ICON_SIZE))
                            .child(self.query.clone()),
                    )
                    .child(
                        page::icon_action("create-note", "New note", true)
                            .debug_selector(|| "create-note".into())
                            .child(icon("plus", page::ICON_SIZE))
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    ),
            )
            .child(
                div()
                    .id("notes-list")
                    .debug_selector(|| "notes-list".into())
                    .role(Role::ListBox)
                    .aria_label("Saved notes")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.list_scroll)
                    .p(px(4.))
                    .when(visible.is_empty(), |d| {
                        d.child(page::meta().p(px(12.)).child(if self.loading {
                            "Loading notes…"
                        } else if !query.trim().is_empty() {
                            "No matching notes"
                        } else {
                            "No notes yet. Save a turn from the transcript, or create one here."
                        }))
                    })
                    .children(
                        {
                            // One shared project tells the reader nothing per row.
                            let one_project = visible
                                .windows(2)
                                .all(|w| w[0].source_cwd == w[1].source_cwd);
                            visible
                                .iter()
                                .enumerate()
                                .map(move |(ix, note)| (ix, note, one_project))
                        }
                        .map(|(ix, note, one_project)| {
                            let id = note.id.clone();
                            let active = self.selected.as_ref() == Some(&id);
                            let preview = note.preview();
                            page::row(("saved-note", ix), note.title.clone(), active)
                                .aria_selected(active)
                                .mb(px(2.))
                                .when(!one_project, |d| {
                                    d.child(
                                        page::meta()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(div().flex_1().truncate().when_some(
                                                note.project_name(),
                                                |d, name| {
                                                    d.child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap_1p5()
                                                            .child(
                                                                self.project_mark(
                                                                    note.source_cwd
                                                                        .as_deref()
                                                                        .unwrap_or(""),
                                                                    12.,
                                                                ),
                                                            )
                                                            .child(name),
                                                    )
                                                },
                                            ))
                                            .child(relative_time(note.updated_at)),
                                    )
                                })
                                .child(
                                    page::label()
                                        .when(!one_project, |d| d.mt_1())
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .when(self.recordings.contains_key(&note.id), |d| {
                                            d.child(icon("mic", page::ICON_SIZE))
                                        })
                                        .child(div().flex_1().min_w_0().truncate().child(
                                            if note.title.is_empty() {
                                                "Untitled".into()
                                            } else {
                                                note.title.clone()
                                            },
                                        ))
                                        .when(one_project, |d| {
                                            d.child(
                                                page::meta()
                                                    .flex_shrink_0()
                                                    .font_weight(FontWeight::NORMAL)
                                                    .child(relative_time(note.updated_at)),
                                            )
                                        }),
                                )
                                .when(!preview.is_empty(), |d| {
                                    d.child(page::meta().mt_1().truncate().child(preview))
                                })
                                .when(!note.tags.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .mt_1p5()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .overflow_hidden()
                                            .children(note.tags.iter().take(3).map(|tag| {
                                                div()
                                                    .max_w(px(96.))
                                                    .truncate()
                                                    .rounded(px(radius::SMALL))
                                                    .bg(content(0.08))
                                                    .px_1p5()
                                                    .py(px(2.))
                                                    .text_size(px(page::META_SIZE))
                                                    .text_color(page::secondary_ink())
                                                    .child(format!("#{tag}"))
                                            }))
                                            .when(note.tags.len() > 3, |d| {
                                                d.child(
                                                    div()
                                                        .text_size(px(page::META_SIZE))
                                                        .text_color(page::secondary_ink())
                                                        .child(format!("+{}", note.tags.len() - 3)),
                                                )
                                            }),
                                    )
                                })
                                .on_click(cx.listener({
                                    let id = id.clone();
                                    move |this, _, _, cx| this.select(id.clone(), cx)
                                }))
                                .on_mouse_down(
                                    MouseButton::Middle,
                                    cx.listener(move |this, _, _, cx| {
                                        this.request_open_tab(&id, cx);
                                        cx.stop_propagation();
                                    }),
                                )
                        }),
                    ),
            )
            .when(!narrow, |d| {
                d.child(
                    page::detail_resize_handle(
                        "resize-notes-list",
                        "Resize notes list",
                        self.resize.is_some(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            if event.click_count == 2 {
                                this.list_width = page::LIST_WIDTH;
                            } else {
                                this.resize = Some((event.position.x.as_f32(), this.list_width));
                            }
                            cx.notify();
                        }),
                    ),
                )
            });
        let detail = if let Some(note) = self.draft(cx) {
            self.detail(note, narrow, cx)
        } else {
            page::empty(
                icon("note", 24.),
                if self.pinned.is_some() {
                    "Note unavailable"
                } else {
                    "Select a note"
                },
                if self.pinned.is_some() {
                    "This note was deleted or is not available."
                } else {
                    "Choose a saved note, or create one with the + button."
                },
            )
            .flex_1()
            .into_any_element()
        };
        div()
            .id("notes-view")
            .track_focus(&self.focus)
            .role(Role::Region)
            .aria_label("Notes")
            .size_full()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if key == "enter"
                    && this.title.focus_handle(cx).is_focused(window)
                    && !this.title.read(cx).is_composing()
                {
                    this.flush(cx);
                    window.focus(&this.query.focus_handle(cx), cx);
                    cx.stop_propagation();
                } else if key == "escape" {
                    if this.project_open {
                        this.project_open = false;
                    } else if this.pinned.is_none()
                        && this.is_narrow(window)
                        && !this.show_list
                        && this.selected.is_some()
                    {
                        this.show_list = true;
                        window.focus(&this.query.focus_handle(cx), cx);
                    } else {
                        this.flush(cx);
                        if !this.dirty {
                            cx.emit(Event::Close);
                        }
                    }
                    cx.stop_propagation();
                    cx.notify();
                } else if (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
                    && key == "s"
                {
                    this.flush(cx);
                    cx.stop_propagation();
                } else if (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
                    && key == "n"
                    && this.pinned.is_none()
                {
                    this.create(window, cx);
                    cx.stop_propagation();
                } else if key == "backspace"
                    && this.tag.focus_handle(cx).is_focused(window)
                    && this.tag.read(cx).content.is_empty()
                    && let Some(tag) = this.draft(cx).and_then(|note| note.tags.last().cloned())
                {
                    this.remove_tag(&tag, cx);
                }
            }))
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    if let Some((start, width)) = this.resize {
                        this.list_width = (width + event.position.x.as_f32() - start).clamp(
                            page::LIST_MIN,
                            page::LIST_MAX
                                .min(this.available_width(window) * 0.5)
                                .max(page::LIST_MIN),
                        );
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.resize = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.resize = None),
            )
            .when(narrow && show_detail && self.pinned.is_none(), |d| {
                d.child(
                    page::toolbar().child(
                        page::action("notes-back", "Back to notes", true)
                            .debug_selector(|| "notes-back".into())
                            .child(icon("chevron-left", page::ICON_SIZE))
                            .child("Notes")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_list = true;
                                window.focus(&this.query.focus_handle(cx), cx);
                                cx.notify();
                            })),
                    ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .when(show_list, |d| d.child(list))
                    .when(show_detail, |d| {
                        d.child(page::detail_pane(!narrow && show_list).child(detail))
                    }),
            )
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("note-error")
                        .role(Role::Alert)
                        .flex_shrink_0()
                        .px_4()
                        .py_2()
                        .flex()
                        .when(self.save_conflict, |d| d.flex_wrap())
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(vorkspace_ui::theme::danger())
                        .child(div().flex_1().child(error))
                        .when(!self.save_conflict, |d| {
                            d.child(
                                div()
                                    .id("retry-note-save")
                                    .role(Role::Button)
                                    .map(vorkspace_ui::theme::tab_reachable)
                                    .aria_label("Retry saving note")
                                    .underline()
                                    .child("Retry")
                                    .on_click(cx.listener(|this, _, _, cx| this.flush(cx))),
                            )
                        })
                        .when(self.save_conflict, |d| {
                            d.child(
                                page::secondary("note-save-copy", "Save a Copy", false, false)
                                    .debug_selector(|| "note-save-copy".into())
                                    .child("Save a Copy")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.resolve_save_conflict(true, cx)
                                    })),
                            )
                            .child(
                                page::secondary(
                                    "note-reload-latest",
                                    "Discard and Reload",
                                    false,
                                    false,
                                )
                                .debug_selector(|| "note-reload-latest".into())
                                .child("Discard and Reload")
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.resolve_save_conflict(false, cx),
                                )),
                            )
                        }),
                )
            })
            .when(self.project_open, |d| d.child(self.project_picker(cx)))
    }
}

fn project_name(path: &str) -> String {
    vorkspace_core::files::leaf_name(path)
}
fn relative_time(at: i64) -> String {
    let seconds = (vorkspace_core::time::now_ms() as i64 - at).max(0) / 1000;
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        86400..604800 => format!("{}d", seconds / 86400),
        _ => chrono::DateTime::from_timestamp_millis(at)
            .map(|date| date.format("%b %-d").to_string())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod screenshots;

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn store() -> Database {
        std::sync::Arc::new(std::sync::Mutex::new(
            vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
        ))
    }

    fn view(
        cx: &mut TestAppContext,
        store: &Database,
    ) -> (gpui::WindowHandle<Notes>, VisualTestContext) {
        cx.update(|cx| {
            crate::input::bind(cx);
            vorkspace_ui::clock::use_test_clock(cx);
        });
        let store = store.clone();
        let window = cx.add_window(move |_, cx| Notes::new(store, cx));
        let visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        (window, visual)
    }

    #[gpui::test]
    fn stale_note_edits_are_preserved_without_overwriting_the_other_window(
        cx: &mut TestAppContext,
    ) {
        let store = store();
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: "shared".into(),
                title: "Shared note".into(),
                body: "Original body".into(),
                ..Default::default()
            })
            .unwrap();
        let (first, mut first_visual) = view(cx, &store);
        let (second, mut second_visual) = view(cx, &store);
        for (window, visual, text) in [
            (first, &mut first_visual, "The first editor's body"),
            (second, &mut second_visual, "The second editor's body"),
        ] {
            window
                .update(visual, |notes, _, cx| {
                    notes
                        .body
                        .update(cx, |body, cx| body.set_text(text.into(), cx));
                    notes.edited(cx);
                    notes.flush(cx);
                })
                .unwrap();
        }
        second_visual.refresh().unwrap();
        second
            .update(&mut second_visual, |notes, _, cx| {
                assert!(notes.save_conflict && notes.dirty);
                assert!(!notes.prepare_close(cx));
                assert_eq!(notes.body.read(cx).content, "The second editor's body");
            })
            .unwrap();
        let button = second_visual.debug_bounds("note-save-copy").unwrap();
        second_visual.simulate_click(button.center(), Default::default());
        second_visual.run_until_parked();

        {
            let db = locked(&store).unwrap();
            let notes = db.list_notes().unwrap();
            assert_eq!(notes.len(), 2);
            assert_eq!(
                db.get_note("shared").unwrap().unwrap().body,
                "The first editor's body"
            );
            let copy = notes.iter().find(|note| note.id != "shared").unwrap();
            assert_eq!(copy.title, "Shared note (copy)");
            assert_eq!(copy.body, "The second editor's body");
            db.delete_note("shared").unwrap();
        }
        second
            .update(&mut second_visual, |notes, _, cx| {
                notes.body.update(cx, |body, cx| {
                    body.set_text("Saved note was deleted".into(), cx)
                });
                notes.edited(cx);
                notes.flush(cx);
                assert!(notes.save_conflict);
            })
            .unwrap();
        assert!(
            locked(&store)
                .unwrap()
                .get_note("shared")
                .unwrap()
                .is_none()
        );
        second_visual.refresh().unwrap();
        let button = second_visual.debug_bounds("note-reload-latest").unwrap();
        second_visual.simulate_click(button.center(), Default::default());
        second_visual.run_until_parked();
        second
            .update(&mut second_visual, |notes, _, _| {
                assert!(!notes.save_conflict);
                assert!(!notes.dirty);
            })
            .unwrap();
    }

    #[gpui::test]
    fn list_resize_highlight_follows_the_curved_detail_edge(cx: &mut TestAppContext) {
        let (window, mut visual) = view(cx, &store());
        visual.simulate_resize(gpui::size(px(1200.), px(800.)));
        visual.run_until_parked();
        let handle = visual.debug_bounds("resize-notes-list").unwrap();
        visual.simulate_mouse_move(handle.center(), None, Default::default());
        visual.run_until_parked();
        window
            .update(&mut visual, |_, window, _| {
                let scale = window.scale_factor();
                let edge = window
                    .painted_quads()
                    .into_iter()
                    .find(|quad| {
                        (quad.bounds.left().0 - handle.center().x.as_f32() * scale).abs() < 0.5
                            && quad.border_widths.left.0 > 0.
                            && (quad.border_color.a - 0.25).abs() < 0.01
                    })
                    .expect("hover paints the curved outline");
                assert_eq!(edge.corner_radii.top_left.0, radius::SURFACE * scale);
                assert_eq!(edge.corner_radii.bottom_left.0, radius::SURFACE * scale);
                assert_eq!(edge.background.as_solid().unwrap().a, 0.);
            })
            .unwrap();
    }

    fn pinned_view(
        cx: &mut TestAppContext,
        store: &Database,
        id: &str,
    ) -> (gpui::WindowHandle<Notes>, VisualTestContext) {
        cx.update(|cx| {
            crate::input::bind(cx);
            vorkspace_ui::clock::use_test_clock(cx);
        });
        let store = store.clone();
        let id = id.to_owned();
        let window = cx.add_window(move |_, cx| Notes::open_note_tab(store, id, cx));
        let visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        (window, visual)
    }

    #[gpui::test]
    fn pinned_editor_flushes_its_draft_when_the_app_quits(cx: &mut TestAppContext) {
        let store = store();
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: "quit-note".into(),
                title: "Quit note".into(),
                body: "Before quit".into(),
                ..Default::default()
            })
            .unwrap();
        let (window, mut visual) = pinned_view(cx, &store, "quit-note");
        window
            .update(&mut visual, |notes, _, cx| {
                notes.body.update(cx, |input, cx| {
                    input.set_text("Unfinished at quit".into(), cx)
                });
                notes.input_changed(cx);
                assert!(notes.dirty);
            })
            .unwrap();
        assert_eq!(
            locked(&store)
                .unwrap()
                .get_note("quit-note")
                .unwrap()
                .unwrap()
                .body,
            "Before quit"
        );
        cx.quit();
        assert_eq!(
            locked(&store)
                .unwrap()
                .get_note("quit-note")
                .unwrap()
                .unwrap()
                .body,
            "Unfinished at quit"
        );
    }

    #[gpui::test]
    fn failed_note_save_keeps_the_pinned_editor_for_retry(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("notes.db");
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            vorkspace_core::store::Store::open(&path).unwrap(),
        ));
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: "close-note".into(),
                title: "Close note".into(),
                body: "Saved body".into(),
                ..Default::default()
            })
            .unwrap();
        let (window, mut visual) = pinned_view(cx, &store, "close-note");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_note_close BEFORE UPDATE ON notes \
                 BEGIN SELECT RAISE(ABORT, 'simulated save failure'); END;",
            )
            .unwrap();
        window
            .update(&mut visual, |notes, _, cx| {
                notes
                    .body
                    .update(cx, |input, cx| input.set_text("Unfinished body".into(), cx));
                notes.input_changed(cx);
                assert!(!notes.prepare_close(cx));
                assert!(notes.dirty);
                assert!(
                    notes
                        .error
                        .as_deref()
                        .is_some_and(|error| error.contains("simulated save failure"))
                );
                assert_eq!(notes.body.read(cx).content, "Unfinished body");
            })
            .unwrap();
        assert_eq!(
            locked(&store)
                .unwrap()
                .get_note("close-note")
                .unwrap()
                .unwrap()
                .body,
            "Saved body"
        );
        connection
            .execute_batch("DROP TRIGGER reject_note_close;")
            .unwrap();
        window
            .update(&mut visual, |notes, _, cx| {
                assert!(notes.prepare_close(cx));
                assert!(!notes.dirty);
            })
            .unwrap();
        assert_eq!(
            locked(&store)
                .unwrap()
                .get_note("close-note")
                .unwrap()
                .unwrap()
                .body,
            "Unfinished body"
        );
    }

    #[gpui::test]
    fn external_write_gate_tracks_only_the_note_with_a_local_draft(cx: &mut TestAppContext) {
        let store = store();
        for id in ["first", "second"] {
            locked(&store)
                .unwrap()
                .upsert_note(&notes::NoteUpsert {
                    id: id.into(),
                    title: id.into(),
                    body: format!("Original {id}"),
                    ..Default::default()
                })
                .unwrap();
        }
        let (first, mut first_visual) = pinned_view(cx, &store, "first");
        let (second, mut second_visual) = pinned_view(cx, &store, "second");
        first
            .update(&mut first_visual, |notes, _, cx| {
                notes.body.update(cx, |input, cx| {
                    input.set_text("Draft before Changed event".into(), cx)
                });
                assert!(notes.blocks_external_write("first", cx));
                assert!(!notes.blocks_external_write("second", cx));
            })
            .unwrap();
        second
            .update(&mut second_visual, |notes, _, cx| {
                assert!(!notes.blocks_external_write("second", cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn pinned_notes_keep_independent_editors_selections_and_drafts(cx: &mut TestAppContext) {
        let store = store();
        let lines = (0..90)
            .map(|index| format!("Line {index}: this note has enough content to scroll."))
            .collect::<Vec<_>>()
            .join("\n");
        let first_draft = format!("First draft\n{lines}");
        let second_draft = format!("Second draft\n{lines}");
        for (id, title) in [("first", "First note"), ("second", "Second note")] {
            locked(&store)
                .unwrap()
                .upsert_note(&notes::NoteUpsert {
                    id: id.into(),
                    title: title.into(),
                    body: format!("Original {id}\n{lines}"),
                    ..Default::default()
                })
                .unwrap();
        }
        let (first, mut first_visual) = pinned_view(cx, &store, "first");
        let (second, mut second_visual) = pinned_view(cx, &store, "second");
        first_visual.simulate_resize(gpui::size(px(600.), px(480.)));
        second_visual.simulate_resize(gpui::size(px(600.), px(480.)));
        let first_body = first
            .update(&mut first_visual, |notes, _, cx| {
                assert_eq!(notes.pane_note_id().as_deref(), Some("first"));
                assert_eq!(notes.pane_title(cx), "First note");
                notes.source = true;
                notes.modes.insert("first".into(), true);
                notes.body.update(cx, |input, cx| {
                    input.set_text(first_draft.clone(), cx);
                    input.select_utf16(0..5, cx);
                });
                notes.body.clone()
            })
            .unwrap();
        first_visual.refresh().unwrap();
        first
            .update(&mut first_visual, |notes, _, _| {
                notes
                    .detail_scroll
                    .set_offset(gpui::point(px(0.), px(-120.)));
            })
            .unwrap();
        let second_body = second
            .update(&mut second_visual, |notes, _, cx| {
                assert_eq!(notes.pane_note_id().as_deref(), Some("second"));
                assert_eq!(notes.pane_title(cx), "Second note");
                notes.source = true;
                notes.modes.insert("second".into(), true);
                notes.body.update(cx, |input, cx| {
                    input.set_text(second_draft.clone(), cx);
                    input.select_utf16(7..12, cx);
                });
                notes.body.clone()
            })
            .unwrap();
        second_visual.refresh().unwrap();
        second
            .update(&mut second_visual, |notes, _, _| {
                notes
                    .detail_scroll
                    .set_offset(gpui::point(px(0.), px(-240.)));
            })
            .unwrap();
        assert_ne!(first_body, second_body);
        first
            .update(&mut first_visual, |notes, _, cx| {
                assert!(notes.dirty);
                notes.select("second".into(), cx);
                assert_eq!(notes.selected.as_deref(), Some("first"));
                assert_eq!(notes.body.read(cx).content, first_draft);
                assert_eq!(notes.body.read(cx).selection_utf16(), 0..5);
                assert_eq!(notes.detail_scroll.offset().y, px(-120.));
                notes.refresh(cx);
            })
            .unwrap();
        first_visual.run_until_parked();
        second
            .update(&mut second_visual, |notes, _, cx| {
                assert!(notes.dirty);
                assert_eq!(notes.selected.as_deref(), Some("second"));
                assert_eq!(notes.body.read(cx).content, second_draft);
                assert_eq!(notes.body.read(cx).selection_utf16(), 7..12);
                assert_eq!(notes.detail_scroll.offset().y, px(-240.));
            })
            .unwrap();
        assert_eq!(
            locked(&store)
                .unwrap()
                .get_note("first")
                .unwrap()
                .unwrap()
                .body,
            first_draft
        );
        assert_eq!(
            locked(&store)
                .unwrap()
                .get_note("second")
                .unwrap()
                .unwrap()
                .body,
            format!("Original second\n{lines}")
        );
    }

    #[gpui::test]
    fn pinned_note_renders_detail_without_library_or_back_navigation(cx: &mut TestAppContext) {
        let store = store();
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: "reading".into(),
                title: "Reading".into(),
                body: "A saved document".into(),
                ..Default::default()
            })
            .unwrap();
        let (_, mut pinned_visual) = pinned_view(cx, &store, "reading");
        pinned_visual.simulate_resize(gpui::size(px(1100.), px(800.)));
        assert!(pinned_visual.debug_bounds("note-detail").is_some());
        assert!(pinned_visual.debug_bounds("notes-list").is_none());
        assert!(pinned_visual.debug_bounds("create-note").is_none());
        assert!(pinned_visual.debug_bounds("open-note-tab").is_none());
        pinned_visual.simulate_resize(gpui::size(px(500.), px(800.)));
        assert!(pinned_visual.debug_bounds("note-detail").is_some());
        assert!(pinned_visual.debug_bounds("notes-back").is_none());

        let (_, mut library_visual) = view(cx, &store);
        library_visual.simulate_resize(gpui::size(px(1100.), px(800.)));
        assert!(library_visual.debug_bounds("notes-list").is_some());
        assert!(library_visual.debug_bounds("open-note-tab").is_some());
    }

    #[gpui::test]
    fn library_note_uses_narrow_tile_width_in_wide_window(cx: &mut TestAppContext) {
        let store = store();
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: "tile-note".into(),
                title: "Tile note".into(),
                body: "Original body".into(),
                ..Default::default()
            })
            .unwrap();
        let (window, mut visual) = view(cx, &store);
        visual.simulate_resize(gpui::size(px(1100.), px(700.)));
        window
            .update(&mut visual, |notes, _, cx| {
                notes.select("tile-note".into(), cx);
                notes.set_pane_width(420., cx);
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("notes-list").is_none());
        assert!(visual.debug_bounds("note-detail").is_some());
        assert!(visual.debug_bounds("notes-back").is_some());

        let back = visual.debug_bounds("notes-back").unwrap().center();
        visual.simulate_click(back, gpui::Modifiers::none());
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("notes-list").is_some());
        assert!(visual.debug_bounds("note-detail").is_none());
        window
            .update(&mut visual, |notes, _, cx| {
                assert_eq!(notes.selected.as_deref(), Some("tile-note"));
                notes.set_pane_width(1000., cx);
            })
            .unwrap();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("notes-list").is_some());
        assert!(visual.debug_bounds("note-detail").is_some());
        assert!(visual.debug_bounds("notes-back").is_none());
    }

    #[gpui::test]
    fn selected_library_editor_can_become_a_pinned_tab_without_reset(cx: &mut TestAppContext) {
        let store = store();
        locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: "handoff".into(),
                title: "Handoff".into(),
                body: "Original".into(),
                ..Default::default()
            })
            .unwrap();
        let (window, mut visual) = view(cx, &store);
        window
            .update(&mut visual, |notes, _, cx| {
                let body = notes.body.clone();
                let document = notes.document.clone();
                let recording = notes.recording.clone();
                notes.body.update(cx, |input, cx| {
                    input.set_text("Unfinished handoff".into(), cx);
                    input.select_utf16(0..10, cx);
                });
                notes
                    .detail_scroll
                    .set_offset(gpui::point(px(0.), px(-80.)));
                assert!(notes.pin_selected_note("handoff", cx));
                assert_eq!(notes.body, body);
                assert_eq!(notes.document, document);
                assert_eq!(notes.recording, recording);
                assert_eq!(notes.body.read(cx).content, "Unfinished handoff");
                assert_eq!(notes.body.read(cx).selection_utf16(), 0..10);
                assert_eq!(notes.detail_scroll.offset().y, px(-80.));
                assert_eq!(notes.pane_note_id().as_deref(), Some("handoff"));
            })
            .unwrap();
        assert!(visual.debug_bounds("note-detail").is_some());
        assert!(visual.debug_bounds("notes-list").is_none());
    }

    #[gpui::test]
    fn library_routes_an_open_note_to_its_existing_tab(cx: &mut TestAppContext) {
        let store = store();
        for id in ["first", "second"] {
            locked(&store)
                .unwrap()
                .upsert_note(&notes::NoteUpsert {
                    id: id.into(),
                    title: id.into(),
                    body: format!("Body of {id}"),
                    ..Default::default()
                })
                .unwrap();
        }
        let (window, mut visual) = view(cx, &store);
        let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        window
            .update(&mut visual, |notes, _, cx| {
                let events = opened.clone();
                let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Event, _| {
                    if let Event::OpenTab(id) = event {
                        events.borrow_mut().push(id.clone());
                    }
                });
                notes._subscriptions.push(subscription);
                notes.set_open_note_tabs(HashSet::from(["first".into()]), cx);
                assert_eq!(notes.selected_note_id().as_deref(), Some("second"));
                notes.select("first".into(), cx);
                assert_eq!(notes.selected_note_id().as_deref(), Some("second"));
                notes.set_open_note_tabs(HashSet::from(["first".into(), "second".into()]), cx);
                assert_eq!(notes.selected_note_id(), None);
                assert!(!notes.recording.read(cx).is_for_note("second"));
            })
            .unwrap();
        visual.run_until_parked();
        assert_eq!(&*opened.borrow(), &["first".to_owned()]);
    }

    #[gpui::test]
    fn deleted_pinned_note_does_not_show_another_document(cx: &mut TestAppContext) {
        let store = store();
        for id in ["first", "second"] {
            locked(&store)
                .unwrap()
                .upsert_note(&notes::NoteUpsert {
                    id: id.into(),
                    title: id.into(),
                    body: format!("Body of {id}"),
                    ..Default::default()
                })
                .unwrap();
        }
        let (window, mut visual) = pinned_view(cx, &store, "first");
        locked(&store).unwrap().delete_note("first").unwrap();
        window
            .update(&mut visual, |notes, _, cx| notes.refresh(cx))
            .unwrap();
        visual.run_until_parked();
        window
            .update(&mut visual, |notes, _, cx| {
                assert_eq!(notes.pane_note_id().as_deref(), Some("first"));
                assert_eq!(notes.selected.as_deref(), Some("first"));
                assert!(notes.draft(cx).is_none());
                assert_eq!(notes.pane_title(cx), "Notes");
            })
            .unwrap();
        assert!(visual.debug_bounds("notes-list").is_none());
    }

    #[gpui::test]
    fn returning_from_the_list_retains_the_note_editor_and_reading_position(
        cx: &mut TestAppContext,
    ) {
        let store = store();
        let (window, mut visual) = view(cx, &store);
        window
            .update(&mut visual, |notes, window, cx| {
                notes.create(window, cx);
                notes.body.update(cx, |input, cx| {
                    input.set_text("Unfinished 🦊 note".into(), cx);
                    input.select_utf16(11..13, cx);
                });
            })
            .unwrap();
        // Changed subscriptions publish after the enclosing app update.
        window
            .update(&mut visual, |notes, _, cx| {
                let id = notes.selected.clone().unwrap();
                let editor = notes.body.clone();
                let position = gpui::point(px(0.), px(-160.));
                notes.detail_scroll.set_offset(position);
                notes.show_list = true;
                notes.select(id.clone(), cx);
                assert!(!notes.show_list);
                assert_eq!(notes.selected.as_ref(), Some(&id));
                assert_eq!(notes.body, editor);
                assert_eq!(notes.body.read(cx).content, "Unfinished 🦊 note");
                assert_eq!(notes.body.read(cx).selection_utf16(), 11..13);
                assert_eq!(notes.detail_scroll.offset(), position);
                assert!(notes.dirty);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_new_note_can_start_with_an_image_and_reopen(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("notes.db");
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            vorkspace_core::store::Store::open(&path).unwrap(),
        ));
        let image = root.path().join("first.svg");
        std::fs::write(
            &image,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#,
        )
        .unwrap();
        let (window, mut visual) = view(cx, &store);
        let id = window
            .update(&mut visual, |notes, window, cx| {
                notes.create(window, cx);
                let id = notes.selected.clone().unwrap();
                assert!(locked(&store).unwrap().get_note(&id).unwrap().is_none());
                notes.add_images(vec![image], cx);
                notes.create(window, cx);
                assert!(
                    notes.notes.iter().any(|note| note.id == id),
                    "pending image belongs to its original draft"
                );
                id
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(&mut visual, |notes, _, cx| {
                assert!(notes.error.is_none(), "{:?}", notes.error);
                assert!(!notes.ephemeral.contains(&id));
                notes.select(id.clone(), cx);
                notes.flush(cx);
            })
            .unwrap();
        let reopened = vorkspace_core::store::Store::open(&path).unwrap();
        let saved = reopened.get_note(&id).unwrap().unwrap();
        assert_eq!(saved.body.matches("![").count(), 1);
        assert!(saved.body.contains("/note-assets/"));
        assert_eq!(
            std::fs::read_dir(root.path().join("note-assets").join(id))
                .unwrap()
                .count(),
            1
        );
    }

    #[gpui::test]
    fn refreshing_the_selected_note_preserves_navigation_and_document(cx: &mut TestAppContext) {
        let store = store();
        let saved = locked(&store)
            .unwrap()
            .upsert_note(&notes::NoteUpsert {
                id: "reading".into(),
                title: "Reading".into(),
                body: "Keep this document".into(),
                ..Default::default()
            })
            .unwrap();
        let (window, mut visual) = view(cx, &store);
        let (scroll, document) = window
            .update(&mut visual, |notes, _, cx| {
                assert_eq!(notes.selected.as_deref(), Some(saved.id.as_str()));
                notes.show_list = true;
                notes
                    .body
                    .update(cx, |input, cx| input.select_utf16(5..9, cx));
                let scroll = notes.detail_scroll.clone();
                let document = notes.document.clone();
                notes.refresh(cx);
                (scroll, document)
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(&mut visual, |notes, _, cx| {
                let position = gpui::point(px(0.), px(-160.));
                scroll.set_offset(position);
                assert_eq!(
                    notes.detail_scroll.offset(),
                    position,
                    "refresh retains the scroll owner"
                );
                assert!(
                    notes.show_list,
                    "background refresh must not close the narrow list"
                );
                assert_eq!(
                    notes.document, document,
                    "unchanged Markdown retains its selection owner"
                );
                assert_eq!(notes.body.read(cx).selection_utf16(), 5..9);
            })
            .unwrap();
    }

    #[gpui::test]
    fn loading_an_empty_list_is_not_an_edit_and_new_note_works(cx: &mut TestAppContext) {
        let store = store();
        let (window, mut visual) = view(cx, &store);
        window
            .update(&mut visual, |notes, window, cx| {
                assert!(!notes.dirty);
                notes.create(window, cx);
                assert_eq!(notes.notes.len(), 1);
                assert_eq!(notes.selected.as_ref(), Some(&notes.notes[0].id));
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(&mut visual, |notes, window, cx| {
                assert!(!notes.dirty, "loading the new note must not look edited");
                notes.create(window, cx);
                assert_eq!(notes.notes.len(), 1, "an untouched draft is discarded");
                assert!(
                    locked(&notes.store)
                        .unwrap()
                        .list_notes()
                        .unwrap()
                        .is_empty()
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn refresh_shows_notes_saved_elsewhere_after_an_empty_first_load(cx: &mut TestAppContext) {
        let store = store();
        let (window, mut visual) = view(cx, &store);
        locked(&store)
            .unwrap()
            .save_note(&vorkspace_core::response::Note {
                id: 7,
                title: "From chat".into(),
                body: "Saved from a response".into(),
                tags: Vec::new(),
                source_session: 0,
                source_cwd: String::new(),
            })
            .unwrap();
        window
            .update(&mut visual, |notes, _, cx| notes.refresh(cx))
            .unwrap();
        visual.run_until_parked();
        window
            .update(&mut visual, |notes, _, _| {
                assert_eq!(notes.notes.len(), 1);
                assert_eq!(notes.notes[0].title, "From chat");
                assert!(!notes.dirty);
            })
            .unwrap();
    }

    #[gpui::test]
    fn deleting_another_note_keeps_the_selected_unsaved_text(cx: &mut TestAppContext) {
        let store = store();
        let saved = |id: &str| {
            locked(&store)
                .unwrap()
                .upsert_note(&notes::NoteUpsert {
                    id: id.into(),
                    title: id.into(),
                    body: "Original".into(),
                    ..Default::default()
                })
                .unwrap()
        };
        let first = saved("first");
        let second = saved("second");
        let (window, mut visual) = view(cx, &store);
        window
            .update(&mut visual, |notes, _, cx| {
                notes.select(second.id.clone(), cx);
                notes.body.update(cx, |input, cx| {
                    input.set_text("My unfinished edit".into(), cx)
                });
            })
            .unwrap();
        window
            .update(&mut visual, |notes, _, cx| {
                assert!(notes.dirty);
                notes.delete_saved_note(&first.id, cx);
                assert!(notes.dirty);
                assert_eq!(notes.selected.as_ref(), Some(&second.id));
                assert_eq!(notes.body.read(cx).content, "My unfinished edit");
            })
            .unwrap();
        assert!(
            locked(&store)
                .unwrap()
                .get_note(&first.id)
                .unwrap()
                .is_none()
        );
    }

    #[gpui::test]
    fn typing_still_marks_the_note_edited_and_saves(cx: &mut TestAppContext) {
        let store = store();
        let (window, mut visual) = view(cx, &store);
        window
            .update(&mut visual, |notes, window, cx| notes.create(window, cx))
            .unwrap();
        visual.run_until_parked();
        window
            .update(&mut visual, |notes, _, cx| {
                notes
                    .body
                    .update(cx, |input, cx| input.set_text("Hello".into(), cx));
            })
            .unwrap();
        window
            .update(&mut visual, |notes, _, _| assert!(notes.dirty))
            .unwrap();
        window
            .update(&mut visual, |notes, _, cx| notes.flush(cx))
            .unwrap();
        let saved = locked(&store).unwrap().list_notes().unwrap();
        assert_eq!(saved[0].body, "Hello");
    }
}

/// The body as the preview shows it: a leading heading that repeats the
/// title the page already shows above it is left out. Source keeps it.
fn without_title_heading<'a>(body: &'a str, title: &str) -> &'a str {
    let trimmed = body.trim_start();
    let Some(rest) = trimmed.strip_prefix("# ") else {
        return body;
    };
    let (line, after) = rest.split_once('\n').unwrap_or((rest, ""));
    if !title.trim().is_empty() && line.trim().eq_ignore_ascii_case(title.trim()) {
        after.trim_start_matches(['\n', '\r'])
    } else {
        body
    }
}

#[cfg(test)]
mod title_tests {
    use super::without_title_heading;

    #[test]
    fn a_heading_that_repeats_the_title_is_left_out_of_the_preview() {
        assert_eq!(
            without_title_heading("# Glide timing\n\nThe thumb springs.", "Glide timing"),
            "The thumb springs."
        );
        assert_eq!(without_title_heading("# Glide timing", "glide timing"), "");
        // Another heading, a subheading or no title keeps the body whole.
        let body = "# Other\n\nText";
        assert_eq!(without_title_heading(body, "Glide timing"), body);
        assert_eq!(
            without_title_heading("## Glide timing\nx", "Glide timing"),
            "## Glide timing\nx"
        );
        assert_eq!(without_title_heading("# Untitled\nx", ""), "# Untitled\nx");
    }
}
