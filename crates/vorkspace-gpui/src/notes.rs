//! The desktop Notes split view, backed by the same notes and image assets.
use vorkspace_ui::radius;
mod editor;
mod recording;
mod view;
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
    Notice(String),
}

#[derive(Default)]
struct Editors(HashMap<usize, Vec<gpui::WeakEntity<Notes>>>);
impl gpui::Global for Editors {}

fn editor_store(store: &Database) -> usize {
    std::sync::Arc::as_ptr(store) as usize
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
        let editor = cx.weak_entity();
        let peers = cx
            .default_global::<Editors>()
            .0
            .entry(editor_store(&store))
            .or_default();
        peers.retain(|peer| peer.upgrade().is_some());
        peers.push(editor);
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
        if self.peer_has_unfinished_note(&id, cx) {
            self.confirm_delete = false;
            self.error = Some(
                "This note has unfinished edits in another window. Save them before deleting it."
                    .into(),
            );
            cx.notify();
            return;
        }
        if self.recordings.contains_key(&id) {
            let expected = self.saved_note.clone();
            self.recording
                .update(cx, |page, cx| page.delete_note_recording(expected, cx));
        } else {
            self.delete_saved_note(&id, cx);
        }
    }

    fn peer_has_unfinished_note(&self, id: &str, cx: &Context<Self>) -> bool {
        let current = cx.weak_entity();
        cx.try_global::<Editors>()
            .and_then(|editors| editors.0.get(&editor_store(&self.store)))
            .into_iter()
            .flatten()
            .filter(|peer| **peer != current)
            .filter_map(gpui::WeakEntity::upgrade)
            .any(|peer| {
                let peer = peer.read(cx);
                peer.image_note.as_deref() == Some(id)
                    || peer.recording.read(cx).has_pending_note_edit(id)
                    || (peer.selected.as_deref() == Some(id)
                        && (peer.dirty
                            || peer.title.read(cx).is_composing()
                            || peer.body.read(cx).is_composing()
                            || !peer.tag.read(cx).content.is_empty()))
            })
    }

    fn delete_saved_note(&mut self, id: &str, cx: &mut Context<Self>) {
        let selected = self.selected.as_deref() == Some(id);
        let expected = if selected {
            self.saved_note.as_ref()
        } else {
            self.notes
                .iter()
                .find(|note| note.id == id && !self.ephemeral.contains(id))
        };
        let result =
            locked(&self.store).and_then(|store| store.delete_note_if_current(id, expected));
        self.confirm_delete = false;
        match result {
            Ok(true) => self.forget_note(id, cx),
            Ok(false) => {
                self.error = Some(
                    "This note changed in another window. Reload it before deleting it.".into(),
                );
                self.save_conflict |= selected && self.dirty;
            }
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
}

fn project_name(path: &str) -> String {
    vorkspace_core::files::leaf_name(path)
}

#[cfg(test)]
mod screenshots;

#[cfg(test)]
mod tests;

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
