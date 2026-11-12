//! Native FileEditor/FilePane previews. Disk work and syntax parsing stay off
//! the UI thread; drafts, selections and scrolling belong to each live tab.
use crate::{
    git_status::{Checkout, EditorBaselineRevision, ProjectGit},
    input::{self, Input},
    panel_kit::{icon_action, stroke},
    transcript::TranscriptView,
};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Image, ImageFormat,
    KeyBinding, ObjectFit, PromptLevel, Role, SharedString, Subscription, Task, Window, actions,
    div, img, prelude::*, px,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use vorkspace_core::files::document::{self, Document, TextDocument};
use vorkspace_ui::radius;
use vorkspace_ui::theme::*;

mod chrome;
mod diff;
mod document_io;
mod media;
#[cfg(test)]
#[path = "file_editor/tests.rs"]
mod parity_tests;
mod search;
use search::{FindIntent, FindState, SearchOption};
mod selection;

actions!(
    file_editor,
    [
        ToggleAutosave,
        ToggleWordWrap,
        ReloadFile,
        Save,
        Find,
        Replace,
        FindNext,
        FindPrevious,
        DismissSearch,
        MatchCase,
        WholeWord,
        Regex,
        Format,
        PreviousChange,
        NextChange,
        RevertChange
    ]
);
pub(crate) fn bind(cx: &mut App) {
    let m = "cmd";
    cx.bind_keys([
        KeyBinding::new(&format!("{m}-s"), Save, Some("FileEditor")),
        KeyBinding::new(&format!("{m}-f"), Find, Some("FileEditor")),
        KeyBinding::new(&format!("{m}-alt-f"), Replace, Some("FileEditor")),
        KeyBinding::new("alt-shift-f", Format, Some("FileEditor")),
        KeyBinding::new("alt-z", ToggleWordWrap, Some("FileEditor")),
        KeyBinding::new("alt-c", MatchCase, Some("FileEditor")),
        KeyBinding::new("alt-w", WholeWord, Some("FileEditor")),
        KeyBinding::new("alt-r", Regex, Some("FileEditor")),
        KeyBinding::new("f3", FindNext, Some("FileEditor")),
        KeyBinding::new("shift-f3", FindPrevious, Some("FileEditor")),
        KeyBinding::new("alt-up", PreviousChange, Some("FileEditor")),
        KeyBinding::new("alt-down", NextChange, Some("FileEditor")),
        KeyBinding::new(&format!("{m}-g"), FindNext, Some("FileEditor")),
        KeyBinding::new(&format!("{m}-shift-g"), FindPrevious, Some("FileEditor")),
        KeyBinding::new("escape", DismissSearch, Some("FileEditor")),
    ]);
}
pub(crate) enum FileEvent {
    Dirty,
    Saved,
    SaveFailed,
    WordWrap(bool),
    AddToChat(String),
}

struct FileCheckout {
    entity: Entity<Checkout>,
    relative: String,
    revision: Option<EditorBaselineRevision>,
}

pub(crate) struct FileEditor {
    pub path: String,
    cwd: String,
    checkout: Option<FileCheckout>,
    // The baseline is immutable until Git refreshes it. Per-edit analysis
    // and diff actions share its bytes instead of copying the entire file.
    git_original: Option<Arc<str>>,
    git_staged: bool,
    git_task: Option<Task<()>>,
    analysis: Option<Task<()>>,
    /// The Tree-sitter parser and last tree, reused by each incremental pass.
    syntax: Arc<std::sync::Mutex<Option<vorkspace_editor::editor_syntax::Session>>>,
    focus: FocusHandle,
    pub input: Entity<Input>,
    selection: selection::Selection,
    search: Entity<Input>,
    search_open: bool,
    find_state: FindState,
    replacement: Entity<Input>,
    replace_open: bool,
    match_case: bool,
    whole_word: bool,
    regex: bool,
    autosave: Option<Task<()>>,
    autosave_enabled: bool,
    formatting: Option<Task<()>>,
    change_index: usize,
    document: Option<TextDocument>,
    image: Option<Arc<Image>>,
    media: Option<Entity<media::MediaPreview>>,
    media_events: Option<Subscription>,
    svg_preview: Option<Arc<Image>>,
    unsupported: Option<String>,
    error: Option<String>,
    load_error: Option<String>,
    disk_changed: bool,
    disk_revision: Option<document::LocalRevision>,
    load_task: Option<Task<()>>,
    pub saving: bool,
    pub dirty: bool,
    visible: bool,
    preview: bool,
    markdown: Option<Entity<TranscriptView>>,
    markdown_source: SharedString,
    navigation: Option<(usize, usize)>,
    _subscriptions: Vec<Subscription>,
    disk_poll: Option<Task<()>>,
}
impl EventEmitter<FileEvent> for FileEditor {}
impl Focusable for FileEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl FileEditor {
    #[cfg(test)]
    pub(crate) fn new(path: String, git: Entity<ProjectGit>, cx: &mut Context<Self>) -> Self {
        let cwd = git.read(cx).project().to_owned();
        Self::new_in_working_copy(path, cwd, git, false, cx)
    }
    pub(crate) fn new_in_working_copy(
        path: String,
        cwd: String,
        git: Entity<ProjectGit>,
        deferred: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(Input::editor);
        let search = cx.new(|cx| {
            let mut field = Input::field("Find", "Find", content(0.35), cx);
            field.set_font_size(12.);
            field
        });
        let replacement = cx.new(|cx| {
            let mut field = Input::field("Replace", "Replace", content(0.35), cx);
            field.set_font_size(12.);
            field
        });
        input.update(cx, |input, _| input.set_editor_path(path.clone()));
        let prefs = cx.try_global::<vorkspace_ui::preferences::Preferences>();
        let autosave_enabled = prefs.is_some_and(|prefs| prefs.autosave);
        if let Some(wrap) = prefs.map(|prefs| prefs.file_word_wrap) {
            input.update(cx, |input, cx| input.set_word_wrap(wrap, cx));
        }
        let mut subscriptions = vec![
            cx.subscribe(&input, |this, _, _: &input::Changed, cx| this.edited(cx)),
            cx.observe(&input, |this, _, cx| {
                this.observe_code_selection(cx);
                cx.notify();
            }),
            cx.subscribe(&input, |this, _, event: &input::Rejected, cx| {
                this.error = Some(event.0.into());
                cx.notify();
            }),
            cx.subscribe(&search, |this, _, _: &input::Changed, cx| {
                this.find_matches(FindIntent::SelectMatch, cx)
            }),
            cx.subscribe(&search, |this, _, _: &input::Submitted, cx| {
                this.step_match(false, cx)
            }),
            cx.subscribe(&replacement, |this, _, _: &input::Changed, cx| {
                this.find_matches(FindIntent::PreserveSelection, cx)
            }),
            cx.subscribe(&replacement, |this, _, _: &input::Submitted, cx| {
                this.replace_matches(false, cx)
            }),
            cx.observe_global::<vorkspace_ui::preferences::Preferences>(|this, cx| {
                let prefs = cx.global::<vorkspace_ui::preferences::Preferences>();
                let (wrap, autosave) = (prefs.file_word_wrap, prefs.autosave);
                if this.input.read(cx).word_wrap() != wrap {
                    this.input
                        .update(cx, |input, cx| input.set_word_wrap(wrap, cx));
                }
                if this.autosave_enabled != autosave {
                    this.autosave_enabled = autosave;
                    this.schedule_autosave(cx);
                }
            }),
        ];
        let checkout = Path::new(&path)
            .strip_prefix(&cwd)
            .ok()
            .and_then(Path::to_str)
            .filter(|relative| !relative.is_empty() && !cwd.is_empty() && cwd != "~")
            .map(|relative| {
                let entity = ProjectGit::checkout_for(&git, &cwd, cx);
                subscriptions.push(cx.observe(&entity, |this, _, cx| {
                    if this.visible
                        && let Some(checkout) = &this.checkout
                    {
                        let revision = checkout
                            .entity
                            .read(cx)
                            .editor_baseline_revision(&checkout.relative);
                        if revision.is_none() || checkout.revision != revision {
                            this.refresh_git(cx);
                        }
                    }
                }));
                FileCheckout {
                    entity,
                    relative: relative.to_owned(),
                    revision: None,
                }
            });
        let viewer = cx.entity_id();
        cx.on_release(move |this, cx| {
            if this.visible
                && let Some(checkout) = &this.checkout
            {
                checkout
                    .entity
                    .update(cx, |checkout, cx| checkout.poll(viewer, false, cx));
            }
        })
        .detach();
        let mut this = Self {
            path,
            cwd,
            checkout,
            git_original: None,
            git_staged: false,
            git_task: None,
            analysis: None,
            syntax: Default::default(),
            focus: cx.focus_handle(),
            input,
            selection: selection::Selection::default(),
            search,
            search_open: false,
            find_state: FindState::default(),
            replacement,
            replace_open: false,
            match_case: false,
            whole_word: false,
            regex: false,
            autosave: None,
            autosave_enabled,
            formatting: None,
            change_index: 0,
            document: None,
            image: None,
            media: None,
            media_events: None,
            svg_preview: None,
            unsupported: None,
            error: None,
            load_error: None,
            disk_changed: false,
            disk_revision: None,
            load_task: None,
            saving: false,
            dirty: false,
            visible: false,
            preview: false,
            markdown: None,
            markdown_source: "".into(),
            navigation: None,
            _subscriptions: subscriptions,
            disk_poll: None,
        };
        if !deferred {
            this.load(false, cx);
        }
        this
    }
    pub(crate) fn navigate(&mut self, line: usize, column: usize, cx: &mut Context<Self>) {
        self.navigation = Some((line, column));
        self.apply_navigation(cx);
    }
    fn apply_navigation(&mut self, cx: &mut Context<Self>) {
        if self.document.is_none() {
            return;
        }
        let Some((line, column)) = self.navigation.take() else {
            return;
        };
        self.preview = false;
        self.input.update(cx, |input, cx| {
            let mut start = 0;
            for _ in 1..line.min(input.content.len() + 1) {
                if let Some(n) = input.content[start..].find('\n') {
                    start += n + 1;
                } else {
                    start = input.content.len();
                    break;
                }
            }
            let end = input.content[start..]
                .find('\n')
                .map_or(input.content.len(), |n| start + n);
            let at = start
                + input.content[start..end]
                    .char_indices()
                    .nth(column.saturating_sub(1))
                    .map_or(end - start, |(at, _)| at);
            input.select_range(at..at, cx);
        });
        cx.notify();
    }
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        if let Some(checkout) = &self.checkout {
            let viewer = cx.entity_id();
            checkout
                .entity
                .update(cx, |checkout, cx| checkout.poll(viewer, visible, cx));
        }
        if !visible {
            self.selection.dismiss();
            self.disk_poll = None;
        }
        if let Some(media) = &self.media {
            media.update(cx, |m, _| m.set_visible(visible));
        }
        if visible {
            self.load(false, cx);
            self.refresh_git(cx);
            self.disk_poll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    if this.update(cx, |this, cx| this.load(false, cx)).is_err() {
                        break;
                    }
                }
            }));
        }
    }
    fn is_markdown(&self) -> bool {
        matches!(
            PathBuf::from(&self.path)
                .extension()
                .and_then(|s| s.to_str()),
            Some("md" | "mdx" | "markdown")
        )
    }
    fn edited(&mut self, cx: &mut Context<Self>) {
        self.selection.dismiss();
        let Some(doc) = &self.document else { return };
        let dirty = self.input.read(cx).content.as_ref() != doc.text;
        if self.dirty != dirty {
            self.dirty = dirty;
            cx.emit(FileEvent::Dirty);
        }
        self.schedule_analysis(cx);
        self.schedule_autosave(cx);
        if self.dirty {
            self.input
                .update(cx, |input, cx| input.schedule_completion(false, cx));
        }
        self.find_matches(FindIntent::PreserveSelection, cx);
        cx.notify();
    }
    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(checkout) = &mut self.checkout else {
            return;
        };
        checkout.revision = checkout
            .entity
            .read(cx)
            .editor_baseline_revision(&checkout.relative);
        let cwd = PathBuf::from(&self.cwd);
        let relative = checkout.relative.clone();
        let work = cx.background_executor().spawn(async move {
            let (original, staged) =
                vorkspace_core::scm::editor_baseline(&cwd, &relative).ok()??;
            Some((
                Arc::<str>::from(original.replace("\r\n", "\n").replace('\r', "\n")),
                staged,
            ))
        });
        self.git_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let (original, staged) =
                    result.map_or((None, false), |(original, staged)| (Some(original), staged));
                let side_changed = this.git_staged != staged;
                this.git_staged = staged;
                if this.git_original != original {
                    this.git_original = original;
                    this.schedule_analysis(cx);
                }
                if side_changed {
                    cx.notify();
                }
            });
        }));
    }

    fn changed_checkout(&self, cx: &mut Context<Self>) {
        if let Some(checkout) = &self.checkout {
            checkout.entity.update(cx, Checkout::changed);
        }
    }
    fn schedule_analysis(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).content.clone();
        let original = self.git_original.clone();
        let path = self.path.clone();
        let syntax = vorkspace_editor::editor_syntax::supports(&path).then(|| self.syntax.clone());
        self.analysis = Some(cx.spawn(async move |this, cx| {
            // An incremental parse costs about a frame, so native analysis
            // runs per edit. The QuickJS fallback reparses everything, so it
            // waits for a pause in typing.
            if syntax.is_none() {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
            }
            let source = text.clone();
            let base = original.clone();
            let work = cx.background_executor().spawn(async move {
                match syntax {
                    Some(syntax) => vorkspace_editor::editor_syntax::analyze(
                        &syntax,
                        &path,
                        &source,
                        base.as_deref(),
                    ),
                    None => {
                        vorkspace_editor::editor_analysis::analyze(&path, &source, base.as_deref())
                    }
                }
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.input.read(cx).content == text && this.git_original == original {
                    match result {
                        Ok(analysis) => this
                            .input
                            .update(cx, |input, cx| input.set_editor_analysis(analysis, cx)),
                        Err(error) => eprintln!("vorkspace: editor analysis failed: {error}"),
                    }
                }
            });
        }));
    }
    fn toggle_word_wrap(&mut self, cx: &mut Context<Self>) {
        let wrap = !self.input.read(cx).word_wrap();
        self.input
            .update(cx, |input, cx| input.set_word_wrap(wrap, cx));
        cx.emit(FileEvent::WordWrap(wrap));
        cx.notify();
    }
    fn format_document(&mut self, cx: &mut Context<Self>) {
        if self.document.is_none() || self.saving || self.input.read(cx).is_composing() {
            return;
        }
        let source = self.input.read(cx).content.clone();
        let at = self.input.read(cx).selection_range().end;
        let path = self.path.clone();
        self.formatting = Some(cx.spawn(async move |this, cx| {
            let text = source.clone();
            let result = cx
                .background_executor()
                .spawn(async move { vorkspace_editor::editor_analysis::format(&path, &text, at) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.input.read(cx).content == source
                    && !this.input.read(cx).is_composing()
                    && let Ok(Some(result)) = result
                    && result.formatted != source.as_ref()
                {
                    this.input.update(cx, |input, cx| {
                        input.replace_bytes(0..source.len(), &result.formatted, result.cursor, cx)
                    });
                }
            });
        }));
    }
    fn is_svg(&self) -> bool {
        PathBuf::from(&self.path)
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("svg"))
    }
    fn update_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview && self.is_svg() {
            self.svg_preview = Some(Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                self.input.read(cx).content.as_bytes().to_vec(),
            )));
        }
        if self.preview && self.is_markdown() {
            let text = self.input.read(cx).content.clone();
            if self.markdown.is_none() || self.markdown_source != text {
                self.markdown_source = text.clone();
                let base = PathBuf::from(&self.path)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned());
                self.markdown = Some(cx.new(|cx| {
                    let mut view = TranscriptView::document(text.to_string(), cx);
                    view.file_link_base = base;
                    view
                }));
            }
        }
    }
    fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.dismiss();
        self.preview = !self.preview;
        crate::preference_writes::remember(cx, |prefs| {
            prefs.file_view_mode = if self.preview { "preview" } else { "source" }.into();
        });
        self.update_preview(cx);
        window.focus(&self.focus_handle(cx), cx);
        cx.notify();
    }
    fn step_change(&mut self, backwards: bool, cx: &mut Context<Self>) {
        let changes = &self.input.read(cx).editor_analysis().changes;
        if changes.is_empty() {
            return;
        }
        self.change_index = if backwards {
            self.change_index.min(changes.len() - 1).saturating_sub(1)
        } else {
            (self.change_index + 1).min(changes.len() - 1)
        };
        let from = changes[self.change_index].from;
        self.input
            .update(cx, |i, cx| i.select_range(from..from, cx));
        cx.notify();
    }
    fn revert_change(&mut self, cx: &mut Context<Self>) {
        self.apply_change(cx);
    }
}
impl gpui::Render for FileEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.document.is_some();
        let controls = ready.then(|| {
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1p5()
                .when(!self.preview, |d| d.child(self.word_wrap_control(cx)))
                .when(self.is_markdown() || self.is_svg(), |d| {
                    d.child(self.view_mode_control(cx))
                })
                .into_any_element()
        });
        let selection_menu = self.code_selection_menu(window, cx);
        if ((!ready || self.preview) && self.input.focus_handle(cx).is_focused(window))
            || (!self.search_open
                && (self.search.focus_handle(cx).is_focused(window)
                    || self.replacement.focus_handle(cx).is_focused(window)))
        {
            window.focus(&self.focus, cx);
        }
        if ready && !self.preview && self.focus.is_focused(window) {
            window.focus(&self.input.focus_handle(cx), cx);
        }
        let preview_image = self
            .image
            .as_ref()
            .or_else(|| self.preview.then_some(self.svg_preview.as_ref()).flatten());
        let body: AnyElement = if let Some(media) = &self.media {
            media.clone().into_any_element()
        } else if let Some(image) = preview_image {
            div()
                .size_full()
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .with_fallback(|| {
                            div()
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .child("Couldn’t decode this image")
                                .into_any_element()
                        }),
                )
                .into_any_element()
        } else if ready {
            if self.preview {
                self.markdown
                    .clone()
                    .map(|m| m.into_any_element())
                    .unwrap_or_else(|| div().into_any_element())
            } else {
                self.input.clone().into_any_element()
            }
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .p_6()
                .text_size(px(12.))
                .text_color(content(0.5))
                .child(
                    self.unsupported
                        .clone()
                        .or(self.error.clone())
                        .or(self.load_error.clone())
                        .unwrap_or_else(|| "Opening file…".into()),
                )
                .when(self.load_task.is_none(), |d| {
                    d.child(
                        div()
                            .id("open-file-externally")
                            .role(Role::Button)
                            .map(vorkspace_ui::theme::tab_reachable)
                            .aria_label("Open externally")
                            .px_3()
                            .py_1()
                            .rounded(px(radius::CONTROL))
                            .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.1)))
                            .child("Open externally")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.open_url(&format!(
                                    "file://{}",
                                    this.path
                                        .replace('%', "%25")
                                        .replace(' ', "%20")
                                        .replace('#', "%23")
                                        .replace('?', "%3F")
                                ))
                            })),
                    )
                })
                .when(self.load_task.is_none(), |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("retry-file")
                                    .role(Role::Button)
                                    .map(vorkspace_ui::theme::tab_reachable)
                                    .aria_label("Retry")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(radius::CONTROL))
                                    .bg(content(0.1))
                                    .child("Retry")
                                    .on_click(cx.listener(|this, _, _, cx| this.load(true, cx))),
                            )
                            .child(
                                div()
                                    .id("reveal-file")
                                    .role(Role::Button)
                                    .map(vorkspace_ui::theme::tab_reachable)
                                    .aria_label("Reveal")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(radius::CONTROL))
                                    .bg(content(0.1))
                                    .child("Reveal")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let path = PathBuf::from(&this.path);
                                        cx.background_executor()
                                            .spawn(async move {
                                                let _ = vorkspace_core::files::reveal_path(&path);
                                            })
                                            .detach();
                                    })),
                            )
                            .child(
                                div()
                                    .id("copy-file-path")
                                    .role(Role::Button)
                                    .map(vorkspace_ui::theme::tab_reachable)
                                    .aria_label("Copy path")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(radius::CONTROL))
                                    .bg(content(0.1))
                                    .child("Copy path")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            this.path.clone(),
                                        ))
                                    })),
                            ),
                    )
                })
                .into_any_element()
        };
        div()
            .id("file-editor")
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .key_context("FileEditor")
            .track_focus(&self.focus)
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.finish_code_selection(window, cx)),
            )
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.selection.dismiss();
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, w, cx| {
                if this.search_open
                    && event.keystroke.key == "enter"
                    && event.keystroke.modifiers.shift
                    && this.search.focus_handle(cx).is_focused(w)
                {
                    this.step_match(true, cx);
                    cx.stop_propagation();
                    return;
                }
                if let Some(media) = &this.media {
                    media.update(cx, |m, cx| m.zoom_key(event, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save(cx)))
            .on_action(cx.listener(|this, _: &ToggleWordWrap, _, cx| this.toggle_word_wrap(cx)))
            .on_action(cx.listener(|this, _: &ReloadFile, w, cx| this.reload(w, cx)))
            .on_action(cx.listener(|this, _: &Find, w, cx| this.open_search(w, cx)))
            .on_action(cx.listener(|this, _: &Replace, w, cx| {
                this.open_search(w, cx);
                this.replace_open = true;
            }))
            .on_action(cx.listener(|this, _: &Format, _, cx| this.format_document(cx)))
            .on_action(cx.listener(|this, _: &MatchCase, _, cx| {
                this.toggle_search_option(SearchOption::Case, cx)
            }))
            .on_action(cx.listener(|this, _: &WholeWord, _, cx| {
                this.toggle_search_option(SearchOption::Word, cx)
            }))
            .on_action(cx.listener(|this, _: &Regex, _, cx| {
                this.toggle_search_option(SearchOption::Regex, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousChange, _, cx| this.step_change(true, cx)))
            .on_action(cx.listener(|this, _: &NextChange, _, cx| this.step_change(false, cx)))
            .on_action(cx.listener(|this, _: &RevertChange, _, cx| this.revert_change(cx)))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.step_match(false, cx)))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.step_match(true, cx)))
            .on_action(cx.listener(|this, _: &DismissSearch, w, cx| this.dismiss_search(w, cx)))
            .child(self.path_header(controls))
            .when(self.search_open, |d| d.child(self.search_panel(cx)))
            .when_some(
                self.error
                    .clone()
                    .or_else(|| self.load_error.clone())
                    .filter(|_| ready)
                    .or_else(|| {
                        self.disk_changed
                            .then(|| "File changed on disk. Reload to use the new version.".into())
                    }),
                |d, error| {
                    d.child(
                        div()
                            .px_3()
                            .py_2()
                            .text_size(px(12.))
                            .text_color(vorkspace_ui::theme::warning())
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().flex_1().child(error))
                            .when(self.disk_changed, |d| {
                                d.child(
                                    div()
                                        .id("file-conflict-reload")
                                        .role(Role::Button)
                                        .map(vorkspace_ui::theme::tab_reachable)
                                        .aria_label("Reload from disk")
                                        .cursor_pointer()
                                        .child("Reload from disk")
                                        .on_click(cx.listener(|this, _, w, cx| this.reload(w, cx))),
                                )
                                .child(
                                    div()
                                        .id("file-conflict-save-mine")
                                        .role(Role::Button)
                                        .map(vorkspace_ui::theme::tab_reachable)
                                        .aria_label("Save my changes")
                                        .cursor_pointer()
                                        .child("Save my changes")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.save_my_changes(cx)),
                                        ),
                                )
                            }),
                    )
                },
            )
            .child(div().flex_1().min_h_0().min_w_0().child(body))
            .when(ready && !self.preview, |d| d.child(self.status_footer(cx)))
            .child(self.selection_surface(cx))
            .children(selection_menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Bounds, EntityInputHandler, TestAppContext, VisualTestContext, WindowBounds, WindowOptions,
        point, size,
    };
    #[gpui::test]
    fn file_overview_tracks_unsaved_syntax_and_staged_or_unstaged_git_baselines(
        cx: &mut TestAppContext,
    ) {
        let root =
            std::env::temp_dir().join(format!("vorkspace-file-overview-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            let result = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        git(&["init"]);
        let path = root.join("sample.js");
        let base = "const value = 1;\n";
        std::fs::write(&path, base).unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "base"]);
        let changed = "const value = ;\n";
        std::fs::write(&path, changed).unwrap();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                let project = cx.new(|cx| ProjectGit::new(root.to_string_lossy().into_owned(), cx));
                cx.new(|cx| FileEditor::new(path.to_string_lossy().into_owned(), project, cx))
            })
            .unwrap()
        });
        window
            .update(cx, |view, _, cx| view.set_visible(true, cx))
            .unwrap();
        let finish_analysis = |cx: &mut TestAppContext| {
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(200));
            cx.run_until_parked();
        };
        finish_analysis(cx);
        window
            .update(cx, |view, w, cx| {
                assert_eq!(view.git_original.as_deref(), Some(base));
                let input = view.input.read(cx);
                assert_eq!(input.editor_analysis().changes.len(), 1);
                assert!(!input.editor_analysis().diagnostics.is_empty());
                view.input.update(cx, |input, cx| {
                    input.select_range(0..input.content.len(), cx);
                    input.replace_text_in_range(None, base, w, cx);
                });
            })
            .unwrap();
        finish_analysis(cx);
        window
            .update(cx, |view, _, cx| {
                assert!(view.dirty);
                assert!(view.input.read(cx).editor_analysis().changes.is_empty());
                assert!(view.input.read(cx).editor_analysis().diagnostics.is_empty());
            })
            .unwrap();
        git(&["add", "."]);
        window
            .update(cx, |view, _, cx| view.refresh_git(cx))
            .unwrap();
        finish_analysis(cx);
        window
            .update(cx, |view, _, _| {
                assert_eq!(
                    view.git_original.as_deref(),
                    Some(base),
                    "staged-only changes compare to HEAD"
                )
            })
            .unwrap();
        std::fs::write(&path, "const value = 2;\n").unwrap();
        window
            .update(cx, |view, _, cx| view.refresh_git(cx))
            .unwrap();
        finish_analysis(cx);
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.git_original.as_deref(),
                    Some(changed),
                    "mixed staged/unstaged changes compare to the index"
                );
                assert_eq!(view.input.read(cx).editor_analysis().changes.len(), 1);
            })
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[gpui::test]
    fn remembered_preview_mode_keeps_focus_and_find_switches_to_source(cx: &mut TestAppContext) {
        for (extension, source_mode) in [("md", false), ("svg", false), ("md", true), ("svg", true)]
        {
            let path = std::env::temp_dir().join(format!(
                "vorkspace-preview-focus-{}.{extension}",
                std::process::id()
            ));
            std::fs::write(
                &path,
                if extension == "md" {
                    "# Preview\n"
                } else {
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"/>"
                },
            )
            .unwrap();
            let file = path.to_string_lossy().into_owned();
            let window = cx.update(|cx| {
                let prefs = vorkspace_ui::preferences::Preferences {
                    file_view_mode: if source_mode { "source" } else { "preview" }.into(),
                    ..Default::default()
                };
                cx.set_global(prefs);
                input::bind(cx);
                bind(cx);
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(600.), px(400.)),
                        ))),
                        ..Default::default()
                    },
                    |w, cx| {
                        let git = cx.new(|cx| ProjectGit::new("/tmp".into(), cx));
                        let view = cx.new(|cx| FileEditor::new(file, git, cx));
                        w.focus(&view.focus_handle(cx), cx);
                        view
                    },
                )
                .unwrap()
            });
            let cx = &mut VisualTestContext::from_window(window.into(), cx);
            cx.run_until_parked();
            window
                .update(cx, |e, w, cx| {
                    assert_eq!(e.preview, !source_mode);
                    assert!(e.focus.contains_focused(w, cx));
                    assert_eq!(e.input.focus_handle(cx).is_focused(w), source_mode);
                })
                .unwrap();
            let m = "cmd";
            cx.simulate_keystrokes(&format!("{m}-f"));
            cx.run_until_parked();
            window
                .update(cx, |e, w, cx| {
                    assert!(e.search_open);
                    assert!(!e.preview);
                    assert!(e.search.focus_handle(cx).is_focused(w));
                })
                .unwrap();
            std::fs::remove_file(path).unwrap();
        }
    }

    #[gpui::test]
    fn editor_load_find_save_and_external_edits_preserve_the_draft(cx: &mut TestAppContext) {
        let path = std::env::temp_dir().join(format!("vorkspace-editor-{}.rs", std::process::id()));
        std::fs::write(&path, "fn hello() {}\r\n// hello 🙂\r\n").unwrap();
        let file = path.to_string_lossy().into_owned();
        let window = cx.update(|cx| {
            input::bind(cx);
            bind(cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(600.), px(400.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    let git = cx.new(|cx| {
                        ProjectGit::new(std::env::temp_dir().to_string_lossy().into_owned(), cx)
                    });
                    cx.new(|cx| FileEditor::new(file, git, cx))
                },
            )
            .unwrap()
        });
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        window
            .update(cx, |e, w, cx| {
                assert!(e.document.is_some());
                assert!(!e.dirty);
                assert_eq!(
                    e.input.read(cx).content.as_ref(),
                    "fn hello() {}\n// hello 🙂\n"
                );
                e.open_search(w, cx);
                e.search.update(cx, |i, cx| i.set_text("hello".into(), cx));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, w, cx| {
                assert_eq!(e.find_state.count(), 2);
                e.step_match(false, cx);
                let input = e.input.read(cx);
                let last = input.content.rfind("hello").unwrap();
                assert_eq!(input.selection_range(), last..last + "hello".len());
                e.dismiss_search(w, cx);
                e.input
                    .update(cx, |i, cx| i.replace_text_in_range(None, "world", w, cx));
                let edited = e.input.read(cx).selection_range();
                e.step_match(false, cx);
                assert_eq!(e.input.read(cx).selection_range(), edited);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, _, cx| {
                assert!(e.dirty);
                e.save(cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fn hello() {}\r\n// world 🙂\r\n"
        );
        window
            .update(cx, |e, w, cx| {
                assert!(!e.dirty);
                e.input
                    .update(cx, |i, cx| i.replace_text_in_range(None, " draft", w, cx));
            })
            .unwrap();
        cx.run_until_parked();
        std::fs::write(&path, "external\n").unwrap();
        window.update(cx, |e, _, cx| e.load(false, cx)).unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, _, cx| {
                assert!(e.disk_changed);
                assert!(e.dirty);
                assert!(e.input.read(cx).content.contains("draft"));
                e.save(cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, _, _| {
                assert!(e.dirty);
                assert!(e.error.as_ref().unwrap().contains("changed on disk"));
            })
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external\n");
        window
            .update(cx, |e, w, cx| {
                e.load(true, cx);
                e.input.update(cx, |input, cx| {
                    input.replace_text_in_range(None, " typed while reloading", w, cx)
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, _, cx| {
                assert!(e.dirty);
                assert!(e.disk_changed);
                assert!(e.input.read(cx).content.contains("typed while reloading"));
            })
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external\n");
        std::fs::write(&path, include_bytes!("../../assets/vorkspace.png")).unwrap();
        window.update(cx, |e, _, cx| e.load(true, cx)).unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, _, _| {
                assert!(e.image.is_some());
                assert!(!e.dirty);
                assert!(e.document.is_none());
            })
            .unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
