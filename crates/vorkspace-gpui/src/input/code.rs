//! The editor shares native selection/IME handling with Input and retains glyph
//! runs for visible lines. Scroll position is independent of the caret.
use super::*;
use gpui::{AnyElement, Task};
use std::{cell::Cell, rc::Rc, sync::Arc};
use vorkspace_editor::editor_analysis::Analysis;
use vorkspace_ui::editor_scrollbar::{Metrics, WIDTH};
use vorkspace_ui::radius;

const TOP_PADDING: f32 = 8.;
const BOTTOM_PADDING: f32 = 32.;

#[derive(Clone)]
struct MarkerHit {
    bounds: Bounds<Pixels>,
    pos: usize,
    diagnostic: bool,
    message: String,
}

pub(super) struct Code {
    suppress_completion: bool,
    document_bytes: Option<usize>,
    path: String,
    completions: Vec<vorkspace_editor::editor_analysis::Completion>,
    completion_index: usize,
    completion_task: Option<Task<()>>,
    completion_source: Option<(SharedString, usize)>,
    geometry: Geometry,
    search_matches: Rc<Vec<Range<usize>>>,
    visible: Vec<(usize, WrappedLine)>,
    reveal: bool,
    max_width: Pixels,
    undo: Vec<(SharedString, Range<usize>)>,
    redo: Vec<(SharedString, Range<usize>)>,
    wrap: bool,
    layout_width: Option<Pixels>,
    analysis: Arc<Analysis>,
    /// The buffer as of the last change, which `analysis` offsets follow.
    text: SharedString,
    rail: Option<Bounds<Pixels>>,
    drag_offset: Option<f32>,
    hovered: bool,
    markers: Vec<MarkerHit>,
    hovered_marker: Option<usize>,
    affinity: Cell<Option<(usize, bool)>>,
}
impl Code {
    fn new() -> Self {
        Self {
            suppress_completion: false,
            document_bytes: None,
            path: String::new(),
            completions: Vec::new(),
            completion_index: 0,
            completion_task: None,
            completion_source: None,
            geometry: Geometry::new(),
            search_matches: Rc::default(),
            visible: Vec::new(),
            reveal: true,
            max_width: px(0.),
            undo: Vec::new(),
            redo: Vec::new(),
            wrap: true,
            layout_width: None,
            analysis: Arc::new(Analysis::default()),
            text: SharedString::default(),
            rail: None,
            drag_offset: None,
            hovered: false,
            markers: Vec::new(),
            hovered_marker: None,
            affinity: Cell::new(None),
        }
    }
}

// Painting borrows these immutable arrays through one frame. Layout changes
// publish new arrays, so the painter never needs a duplicate line index.
#[derive(Clone)]
struct Geometry {
    offsets: Rc<Vec<usize>>,
    tops: Rc<Vec<f32>>,
    folded: Rc<Vec<vorkspace_editor::editor_analysis::Fold>>,
}

impl Geometry {
    fn new() -> Self {
        Self {
            offsets: Rc::new(vec![0]),
            tops: Rc::new(vec![TOP_PADDING, TOP_PADDING + 20.]),
            folded: Rc::default(),
        }
    }

    fn row(&self, offset: usize) -> usize {
        self.offsets
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    }
    fn fold_at(&self, row: usize) -> Option<&vorkspace_editor::editor_analysis::Fold> {
        self.folded.iter().find(|f| self.row(f.from) == row)
    }
    fn hidden(&self, row: usize) -> bool {
        self.folded
            .iter()
            .any(|f| row > self.row(f.from) && row <= self.row(f.to))
    }
    fn display_row(&self, at: usize) -> usize {
        let row = self.row(at);
        self.folded
            .iter()
            .find(|f| row > self.row(f.from) && row <= self.row(f.to))
            .map_or(row, |f| self.row(f.from))
    }
    fn display_offset(&self, row: usize, at: usize) -> usize {
        let start = self.offsets[row];
        if let Some(f) = self.fold_at(row) {
            if at >= f.to {
                return f.from - start + "…".len() + at - f.to;
            }
            if at > f.from {
                return f.from - start;
            }
        }
        at.saturating_sub(start)
    }
    fn source_offset(&self, row: usize, at: usize) -> usize {
        let start = self.offsets[row];
        if let Some(f) = self.fold_at(row) {
            let prefix = f.from - start;
            if at > prefix {
                return f.to + at.saturating_sub(prefix + "…".len());
            }
        }
        start + at
    }
    fn source_end(&self, row: usize, len: usize) -> usize {
        let row = self.fold_at(row).map_or(row, |f| self.row(f.to));
        self.offsets.get(row + 1).map_or(len, |s| s - 1)
    }
    fn gutter(&self) -> f32 {
        let digits = (self.offsets.len().ilog10() + 1) as f32;

        4. + (digits * 7.5 + 12.).max(36.) + 12. + 1. + 6.
    }
    fn top(&self, row: usize, line_height: f32) -> f32 {
        self.tops
            .get(row)
            .copied()
            .unwrap_or(TOP_PADDING + row as f32 * line_height)
    }
    fn total(&self) -> f32 {
        self.tops.last().copied().unwrap_or(TOP_PADDING) + BOTTOM_PADDING
    }
    fn row_at_y(&self, y: f32) -> usize {
        self.tops
            .partition_point(|top| *top <= y)
            .saturating_sub(1)
            .min(self.offsets.len() - 1)
    }
}
impl Input {
    pub(crate) fn editor(cx: &mut Context<Self>) -> Self {
        let mut input = Self::new(cx);
        input.code = Some(Code::new());
        input.code.as_mut().unwrap().wrap = cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .is_none_or(|p| p.file_word_wrap);
        input.label = "File editor".into();
        input.placeholder = "".into();
        input.font_size = 12.5;
        input.line_height = 20.;
        input.secondary_submit = true;
        input
    }
    /// A wrapped editor for Markdown and other documents stored outside the
    /// file editor. Its format owns the byte budget for the whole document.
    pub(crate) fn document_editor(
        label: &'static str,
        path: &str,
        max_bytes: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        use vorkspace_core::files::document::MAX_TEXT_BYTES;
        let mut input = Self::editor(cx);
        let code = input.code.as_mut().unwrap();
        code.document_bytes = Some(max_bytes.min(MAX_TEXT_BYTES));
        code.path = path.into();
        code.wrap = true;
        input.label = label.into();
        input
    }

    pub(crate) fn set_editor_path(&mut self, path: String) {
        if let Some(code) = &mut self.code {
            code.path = path;
        }
    }
    pub(crate) fn selection_range(&self) -> Range<usize> {
        self.selection.clone()
    }
    pub(crate) fn replace_bytes(
        &mut self,
        range: Range<usize>,
        text: &str,
        cursor: usize,
        cx: &mut Context<Self>,
    ) {
        if self.read_only || self.is_composing() || self.clip_range(range.clone()) != range {
            return;
        }
        let Some(end) = self.apply_replacement(&range, text, cx) else {
            return;
        };
        let at = self.clip_offset(range.start + cursor.min(end - range.start));
        self.selection = at..at;
        self.reversed = false;
        if let Some(code) = &mut self.code {
            code.suppress_completion = true;
        }
        cx.emit(Changed);
        cx.notify();
    }
    pub(super) fn unfold_range(&mut self, range: &Range<usize>) {
        if let Some(code) = &mut self.code {
            let before = code.geometry.folded.len();
            Rc::make_mut(&mut code.geometry.folded).retain(|f| {
                !(range.start > f.from && range.start < f.to
                    || range.end > f.from && range.end < f.to)
            });
            if code.geometry.folded.len() != before {
                code.layout_width = None;
            }
        }
    }
    pub(crate) fn set_search_matches(
        &mut self,
        matches: Vec<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(code) = &mut self.code {
            code.search_matches = Rc::new(matches);
        }
        cx.notify();
    }
    pub(crate) fn schedule_completion(&mut self, explicit: bool, cx: &mut Context<Self>) {
        if self.is_composing() || !self.selection.is_empty() {
            return;
        }
        let at = self.cursor();
        if !explicit
            && !self.content[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        {
            return;
        }
        let Some(code) = &mut self.code else {
            return;
        };
        if !explicit && std::mem::take(&mut code.suppress_completion) {
            return;
        }
        let source = self.content.clone();
        let path = code.path.clone();
        code.completion_source = Some((source.clone(), at));
        code.completion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(if explicit {
                    0
                } else {
                    100
                }))
                .await;
            let text = source.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    vorkspace_editor::editor_analysis::complete(&path, &text, at, explicit)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.content == source
                    && this.cursor() == at
                    && !this.is_composing()
                    && let Some(code) = &mut this.code
                    && code.completion_source.as_ref() == Some(&(source, at))
                {
                    code.completions = result.unwrap_or_default();
                    code.completion_index = 0;
                    cx.notify();
                }
            });
        }));
    }
    pub(super) fn clear_completion(&mut self) {
        if let Some(code) = &mut self.code {
            code.completions.clear();
            code.completion_task = None;
            code.completion_source = None;
        }
    }
    pub(super) fn start_completion(
        &mut self,
        _: &Complete,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.schedule_completion(true, cx);
    }
    pub(super) fn accept_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(code) = &self.code else {
            return false;
        };
        if self.is_composing()
            || code.completion_source.as_ref() != Some(&(self.content.clone(), self.cursor()))
        {
            return false;
        }
        let Some(option) = code.completions.get(code.completion_index).cloned() else {
            return false;
        };
        self.replace_bytes(option.from..option.to, &option.insert, option.cursor, cx);
        true
    }
    pub(super) fn completion_step(&mut self, backwards: bool, cx: &mut Context<Self>) -> bool {
        let Some(code) = &mut self.code else {
            return false;
        };
        if code.completions.is_empty() {
            return false;
        }
        code.completion_index = (code.completion_index
            + code.completions.len()
            + if backwards {
                code.completions.len() - 1
            } else {
                1
            })
            % code.completions.len();
        cx.notify();
        true
    }
    pub(super) fn dismiss_completion(
        &mut self,
        _: &DismissCompletion,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(code) = &mut self.code else {
            cx.propagate();
            return;
        };
        if code.completions.is_empty() && code.completion_task.is_none() {
            cx.propagate();
            return;
        }
        code.completions.clear();
        code.completion_task = None;
        code.completion_source = None;
        cx.notify();
    }
    fn expand_emmet(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(code) = &self.code else {
            return false;
        };
        let path = code.path.clone();
        // Expansion is requested explicitly with Tab. The same bounded worker
        // service is used here, and stale results never replace newer typing.
        let source = self.content.clone();
        let at = self.cursor();
        if !matches!(
            std::path::Path::new(&path)
                .extension()
                .and_then(|s| s.to_str()),
            Some("html" | "htm" | "jsx" | "tsx" | "md" | "mdx" | "markdown")
        ) {
            return false;
        }
        let token = source[..at].split_whitespace().last().unwrap_or("");
        if token.is_empty() || token.contains(['<', '=', ';', '"', '\'']) {
            return false;
        }
        cx.spawn(async move |this, cx| {
            let text = source.clone();
            let result = cx
                .background_executor()
                .spawn(async move { vorkspace_editor::editor_analysis::emmet(&path, &text, at) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.content == source && this.cursor() == at && !this.is_composing() {
                    if let Ok(Some(result)) = result {
                        this.replace_bytes(0..source.len(), &result.formatted, result.cursor, cx);
                    } else {
                        this.replace_bytes(at..at, "  ", 2, cx);
                    }
                }
            });
        })
        .detach();
        true
    }
    pub(super) fn completion_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let code = self.code.as_ref()?;
        if code.completions.is_empty()
            || code.completion_source.as_ref() != Some(&(self.content.clone(), self.cursor()))
        {
            return None;
        }
        let p = self.code_position(self.cursor());
        let gutter = code.geometry.gutter();
        let height = (code.completions.len().min(10) as f32 * 22.).min(240.);
        let viewport = self.bounds.map_or(400., |b| b.size.height.as_f32());
        let y = (p.y - self.scroll_y).as_f32();
        let top = if y + self.line_height + height > viewport {
            (y - height).max(0.)
        } else {
            y + self.line_height
        };
        let first = code.completion_index.saturating_sub(9);
        let viewport_width = self.bounds.map_or(400., |b| b.size.width.as_f32() + gutter);
        let width = code
            .completions
            .iter()
            .map(|o| (o.label.chars().count() + o.detail.chars().count()) as f32 * 7.2 + 44.)
            .fold(180., f32::max)
            .min(400.)
            .min((viewport_width - 8.).max(1.));
        let left = (gutter + (p.x - self.scroll_x).as_f32())
            .clamp(4., (viewport_width - width - 4.).max(4.));
        Some(
            div()
                .id("editor-completions")
                .role(Role::List)
                .aria_label("Autocomplete")
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(width))
                .rounded(px(radius::CONTROL))
                .border_1()
                .border_color(vorkspace_ui::theme::content(0.12))
                .bg(vorkspace_ui::theme::shell_fill().opacity(1.))
                .shadow_md()
                .overflow_hidden()
                .py_1()
                .children(
                    code.completions
                        .iter()
                        .enumerate()
                        .skip(first)
                        .take(10)
                        .map(|(n, option)| {
                            div()
                                .id(("completion", n))
                                .role(Role::ListItem)
                                .aria_label(option.label.clone())
                                .px_2()
                                .h(px(22.))
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(12.))
                                .text_color(vorkspace_ui::theme::content(1.))
                                .when(n == code.completion_index, |d| {
                                    d.bg(vorkspace_ui::theme::accent_color().opacity(0.25))
                                })
                                .child(div().text_color(vorkspace_ui::theme::content(0.5)).child(
                                    match option.kind.as_str() {
                                        "function" => "ƒ",
                                        "keyword" => "k",
                                        "variable" => "v",
                                        _ => "·",
                                    },
                                ))
                                .child(option.label.clone())
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .text_color(vorkspace_ui::theme::content(0.5))
                                        .child(option.detail.clone()),
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(code) = &mut this.code {
                                        code.completion_index = n;
                                    }
                                    this.accept_completion(cx);
                                }))
                        }),
                )
                .into_any_element(),
        )
    }
    fn toggle_fold_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(code) = &mut self.code else {
            return;
        };
        if let Some(n) = code
            .geometry
            .folded
            .iter()
            .position(|f| code.geometry.row(f.from) == row)
        {
            Rc::make_mut(&mut code.geometry.folded).remove(n);
        } else if let Some(fold) = code
            .analysis
            .folds
            .iter()
            .find(|f| code.geometry.row(f.from) == row)
            .cloned()
        {
            if self.selection.end > fold.from && self.selection.start < fold.to {
                self.selection = fold.from..fold.from;
            }
            Rc::make_mut(&mut code.geometry.folded).push(fold);
        }
        code.layout_width = None;
        cx.notify();
    }
    pub(super) fn fold(&mut self, _: &FoldCode, _: &mut Window, cx: &mut Context<Self>) {
        let Some(code) = &self.code else {
            cx.propagate();
            return;
        };
        let row = code.geometry.row(self.cursor());
        if !code
            .geometry
            .folded
            .iter()
            .any(|f| code.geometry.row(f.from) == row)
        {
            self.toggle_fold_row(row, cx);
        }
    }
    pub(super) fn unfold(&mut self, _: &UnfoldCode, _: &mut Window, cx: &mut Context<Self>) {
        let Some(code) = &self.code else {
            cx.propagate();
            return;
        };
        let row = code.geometry.display_row(self.cursor());
        if code.geometry.fold_at(row).is_some() {
            self.toggle_fold_row(row, cx);
        }
    }
    pub(super) fn fold_all(&mut self, _: &FoldAll, _: &mut Window, cx: &mut Context<Self>) {
        let Some(code) = &mut self.code else {
            cx.propagate();
            return;
        };
        let mut folded = code.analysis.folds.clone();
        folded.sort_by_key(|f| f.from);
        let mut end = 0;
        folded.retain(|f| {
            if f.from < end {
                false
            } else {
                end = f.to;
                true
            }
        });
        code.geometry.folded = Rc::new(folded);
        code.layout_width = None;
        self.selection = 0..0;
        cx.notify();
    }
    pub(super) fn unfold_all(&mut self, _: &UnfoldAll, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(code) = &mut self.code {
            Rc::make_mut(&mut code.geometry.folded).clear();
            code.layout_width = None;
            cx.notify();
        }
    }
    pub(super) fn code_fold_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) -> bool {
        let (Some(code), Some(bounds)) = (&self.code, self.bounds) else {
            return false;
        };
        if e.position.x < bounds.left() - px(14.)
            || e.position.x >= bounds.left()
            || e.position.y < bounds.top()
            || e.position.y > bounds.bottom()
        {
            return false;
        }
        let row = code
            .geometry
            .row_at_y((e.position.y - bounds.top() + self.scroll_y).as_f32());
        self.toggle_fold_row(row, cx);
        true
    }
    pub(crate) fn word_wrap(&self) -> bool {
        self.code.as_ref().is_some_and(|c| c.wrap)
    }
    pub(crate) fn editor_analysis(&self) -> &Analysis {
        &self.code.as_ref().unwrap().analysis
    }
    pub(crate) fn set_word_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        if let Some(code) = &mut self.code
            && code.wrap != wrap
        {
            code.wrap = wrap;
            code.layout_width = None;
            code.reveal = true;
            if wrap {
                self.scroll_x = px(0.);
            }
            cx.notify();
        }
    }
    pub(crate) fn set_editor_analysis(&mut self, analysis: Analysis, cx: &mut Context<Self>) {
        if let Some(code) = &mut self.code {
            Rc::make_mut(&mut code.geometry.folded).retain(|fold| analysis.folds.contains(fold));
            code.analysis = Arc::new(analysis);
            code.layout_width = None;
            cx.notify();
        }
    }
    pub(crate) fn cursor_location(&self) -> (usize, usize) {
        let at = self.cursor();
        let row = self.code.as_ref().map_or(0, |c| c.geometry.row(at));
        let start = self.code.as_ref().map_or(0, |c| c.geometry.offsets[row]);
        (row + 1, self.content[start..at].chars().count() + 1)
    }
    pub(crate) fn editor_line_count(&self) -> usize {
        self.code
            .as_ref()
            .map_or(1, |code| code.geometry.offsets.len())
    }
    pub(crate) fn reload_file(
        &mut self,
        text: String,
        preserve_viewport: bool,
        cx: &mut Context<Self>,
    ) {
        let selection = self.selection.clone();
        let scroll = point(self.scroll_x, self.scroll_y);
        self.set_text(text, cx);
        self.reset_editor_history();
        if preserve_viewport {
            let clamp = |at: usize| {
                let mut at = at.min(self.content.len());
                while !self.content.is_char_boundary(at) {
                    at -= 1;
                }
                at
            };
            self.selection = clamp(selection.start)..clamp(selection.end);
            self.scroll_x = scroll.x;
            self.scroll_y = scroll.y;
            if let Some(code) = &mut self.code {
                code.reveal = false;
            }
        } else {
            self.select_range(0..0, cx);
        }
    }
    pub(crate) fn reset_editor_history(&mut self) {
        if let Some(code) = &mut self.code {
            code.undo.clear();
            code.redo.clear();
        }
    }
    pub(super) fn accept_code_edit(
        &self,
        range: &Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        use vorkspace_core::files::document::{MAX_LINE_BYTES, MAX_TEXT_BYTES};
        let bytes = self.content.len() - range.len() + text.len();
        if let Some(max_bytes) = self.code.as_ref().and_then(|code| code.document_bytes) {
            if bytes > max_bytes {
                cx.emit(Rejected("Edit exceeds this document's size limit."));
                return false;
            }
            return true;
        }
        if bytes > MAX_TEXT_BYTES {
            cx.emit(Rejected("Edit exceeds the 2 MB file limit."));
            return false;
        }
        let start = self.content[..range.start].rfind('\n').map_or(0, |n| n + 1);
        let end = self.content[range.end..]
            .find('\n')
            .map_or(self.content.len(), |n| range.end + n);
        let changed = format!(
            "{}{}{}",
            &self.content[start..range.start],
            text,
            &self.content[range.end..end]
        );
        if changed.split('\n').any(|line| line.len() > MAX_LINE_BYTES) {
            cx.emit(Rejected("Edit exceeds the 16 KB line limit."));
            return false;
        }
        true
    }
    pub(super) fn map_code_folds(&mut self, range: &Range<usize>, inserted: usize) {
        if let Some(code) = &mut self.code {
            Rc::make_mut(&mut code.geometry.folded).retain_mut(|f| {
                if range.end <= f.from {
                    f.from = f.from + inserted - range.len();
                    f.to = f.to + inserted - range.len();
                    true
                } else {
                    range.start >= f.to
                }
            });
        }
    }
    pub(super) fn clear_folds(&mut self) {
        if let Some(code) = &mut self.code {
            Rc::make_mut(&mut code.geometry.folded).clear();
            code.layout_width = None;
        }
    }
    pub(super) fn folded_boundary(&self, at: usize, backwards: bool) -> Option<usize> {
        self.code.as_ref()?.geometry.folded.iter().find_map(|f| {
            if backwards && at == f.to {
                Some(f.from)
            } else if !backwards && at == f.from {
                Some(f.to)
            } else {
                None
            }
        })
    }
    pub(super) fn code_changed(&mut self) {
        if let Some(code) = &mut self.code {
            code.completions.clear();
            code.completion_task = None;
            code.completion_source = None;
            code.geometry.offsets = Rc::new(
                std::iter::once(0)
                    .chain(self.content.match_indices('\n').map(|(at, _)| at + 1))
                    .collect(),
            );
            code.visible.clear();
            code.reveal = true;
            code.max_width = px(0.);
            code.layout_width = None;
            code.geometry.tops = Rc::new(
                (0..=code.geometry.offsets.len())
                    .map(|row| TOP_PADDING + row as f32 * self.line_height)
                    .collect(),
            );
            // Carry the last results through the edit until fresh analysis
            // lands; clearing them would flash the whole buffer uncoloured.
            let previous = std::mem::replace(&mut code.text, self.content.clone());
            if let Some(edit) =
                vorkspace_editor::editor_analysis::Edit::between(&previous, &self.content)
            {
                code.analysis = Arc::new(code.analysis.remapped(edit));
            }
            code.markers.clear();
            code.affinity.set(None);
        }
    }
    pub(super) fn reveal_code(&mut self) {
        if let Some(code) = &mut self.code {
            code.reveal = true;
        }
    }
    pub(super) fn record_undo(&mut self) {
        if let Some(code) = &mut self.code
            && self.marked.is_none()
        {
            code.undo
                .push((self.content.clone(), self.selection.clone()));
            let mut bytes = 0;
            let mut keep = 0;
            for (text, _) in code.undo.iter().rev().take(100) {
                bytes += text.len();
                if bytes > 8 * 1024 * 1024 {
                    break;
                }
                keep += 1;
            }
            let discard = code.undo.len().saturating_sub(keep.max(1));
            code.undo.drain(..discard);
            code.redo.clear();
        }
    }
    pub(super) fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.history(false, cx);
    }
    pub(super) fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.history(true, cx);
    }
    fn history(&mut self, redo: bool, cx: &mut Context<Self>) {
        let Some(code) = &mut self.code else {
            cx.propagate();
            return;
        };
        if self.marked.is_some() {
            return;
        }
        let source = if redo { &mut code.redo } else { &mut code.undo };
        let Some((text, selection)) = source.pop() else {
            return;
        };
        let target = if redo { &mut code.undo } else { &mut code.redo };
        target.push((self.content.clone(), self.selection.clone()));
        self.clear_folds();
        self.content = text;
        self.selection = selection;
        self.reversed = false;
        self.code_changed();
        cx.emit(Changed);
        cx.notify();
    }
    pub(super) fn indent(&mut self, _: &Indent, w: &mut Window, cx: &mut Context<Self>) {
        if self.code.is_none() {
            cx.propagate();
            return;
        }
        if self.accept_completion(cx) {
            return;
        }
        if self.selection.is_empty() {
            if self.expand_emmet(cx) {
                return;
            }
            self.replace_text_in_range(None, "  ", w, cx);
        } else {
            self.indent_lines(false, w, cx);
        }
    }
    pub(super) fn outdent(&mut self, _: &Outdent, w: &mut Window, cx: &mut Context<Self>) {
        if self.code.is_none() {
            cx.propagate();
            return;
        }
        self.indent_lines(true, w, cx);
    }
    fn indent_lines(&mut self, outdent: bool, w: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        let selection = self.selection.clone();
        let start = self.content[..selection.start]
            .rfind('\n')
            .map_or(0, |at| at + 1);
        let last = if selection.end > selection.start
            && self.content.as_bytes().get(selection.end - 1) == Some(&b'\n')
        {
            selection.end - 1
        } else {
            selection.end
        };
        let end = self.content[last..]
            .find('\n')
            .map_or(self.content.len(), |n| last + n);
        let replacement = self.content[start..end]
            .split('\n')
            .map(|line| {
                if outdent {
                    let remove = if line.starts_with('\t') {
                        1
                    } else {
                        line.bytes().take(4).take_while(|c| *c == b' ').count()
                    };
                    line[remove..].to_owned()
                } else {
                    format!("    {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let removed = (end - start).saturating_sub(replacement.len());
        let inserted = replacement.len();
        self.replace_text_in_range(Some(self.range16(start..end)), &replacement, w, cx);
        if selection.is_empty() {
            self.move_to(selection.start.saturating_sub(removed).max(start), cx);
        } else {
            self.select_range(start..start + inserted, cx);
        }
    }
    pub(super) fn code_position(&self, at: usize) -> Point<Pixels> {
        let code = self.code.as_ref().unwrap();
        let row = code.geometry.display_row(at);
        let p = code
            .visible
            .iter()
            .find(|(ix, _)| *ix == row)
            .map(|(_, line)| {
                line_position(
                    line,
                    code.geometry.display_offset(row, at),
                    self.line_height(),
                    code.affinity.get() == Some((at, true)),
                )
            })
            .unwrap_or_default();
        p + point(px(0.), px(code.geometry.top(row, self.line_height)))
    }
    pub(super) fn code_index(&self, p: Point<Pixels>) -> usize {
        let code = self.code.as_ref().unwrap();
        let row = code.geometry.row_at_y(p.y.as_f32());
        let start = code.geometry.offsets[row];
        let at = code
            .visible
            .iter()
            .find(|(ix, _)| *ix == row)
            .map_or(start, |(_, line)| {
                code.geometry.source_offset(
                    row,
                    line.closest_index_for_position(
                        point(p.x, p.y - px(code.geometry.top(row, self.line_height))),
                        self.line_height(),
                    )
                    .unwrap_or_else(|ix| ix),
                )
            });
        if let Some((_, line)) = code.visible.iter().find(|(ix, _)| *ix == row) {
            let local_y = (p.y.as_f32() - code.geometry.top(row, self.line_height)).max(0.);
            let downstream = line_position(
                line,
                code.geometry.display_offset(row, at),
                self.line_height(),
                false,
            );
            code.affinity.set(Some((
                at,
                (local_y / self.line_height).floor()
                    < (downstream.y.as_f32() / self.line_height).floor(),
            )));
        }
        at
    }
    pub(super) fn code_vertical(&self, down: bool) -> usize {
        let code = self.code.as_ref().unwrap();
        if code.wrap {
            let p = self.code_position(self.cursor());
            return self.code_index(point(
                p.x,
                (p.y + self.line_height() * if down { 1. } else { -1. }).max(px(0.)),
            ));
        }
        let row = code.geometry.row(self.cursor());
        let col = self.content[code.geometry.offsets[row]..self.cursor()]
            .graphemes(true)
            .count();
        let mut row = if down {
            (row + 1).min(code.geometry.offsets.len() - 1)
        } else {
            row.saturating_sub(1)
        };
        while code.geometry.hidden(row) && row > 0 && row + 1 < code.geometry.offsets.len() {
            if down {
                row += 1;
            } else {
                row -= 1;
            }
        }
        let start = code.geometry.offsets[row];
        let end = code
            .geometry
            .offsets
            .get(row + 1)
            .map_or(self.content.len(), |s| s - 1);
        start
            + self.content[start..end]
                .grapheme_indices(true)
                .nth(col)
                .map_or(end - start, |(at, _)| at)
    }
    pub(super) fn code_scroll(
        &mut self,
        e: &gpui::ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(code) = &mut self.code {
            let delta = e.delta.pixel_delta(px(self.line_height));
            let height = self.bounds.map_or(px(0.), |b| b.size.height);
            self.scroll_y = (self.scroll_y - delta.y)
                .max(px(0.))
                .min((px(code.geometry.total()) - height).max(px(0.)));
            self.scroll_x = (self.scroll_x - delta.x).max(px(0.)).min(code.max_width);
            code.reveal = false;
            cx.stop_propagation();
            cx.notify();
        }
    }
    pub(super) fn code_scrollbar_down(
        &mut self,
        e: &MouseDownEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(code) = &mut self.code else {
            return false;
        };
        let Some(rail) = code.rail.filter(|b| b.contains(&e.position)) else {
            return false;
        };
        cx.stop_propagation();
        if let Some(marker) = code
            .markers
            .iter()
            .rev()
            .find(|m| m.bounds.contains(&e.position))
            .cloned()
        {
            if marker.diagnostic {
                self.move_to(marker.pos, cx);
            }
            let code = self.code.as_mut().unwrap();
            let row = code.geometry.row(marker.pos);
            self.scroll_y = px((code.geometry.top(row, self.line_height)
                - rail.size.height.as_f32() / 2.)
                .max(0.));
            code.reveal = false;
            w.focus(&self.focus, cx);
        } else {
            let metrics = Metrics::new(
                self.scroll_y.as_f32(),
                code.geometry.total(),
                rail.size.height.as_f32(),
                rail.size.height.as_f32(),
            );
            if !metrics.scrollable {
                return true;
            }
            let y = (e.position.y - rail.top()).as_f32();
            let offset = if e.position.x >= rail.left() + px(3.)
                && e.position.x <= rail.right() - px(3.)
                && y >= metrics.thumb_top
                && y <= metrics.thumb_top + metrics.thumb_height
            {
                y - metrics.thumb_top
            } else {
                metrics.thumb_height / 2.
            };
            code.drag_offset = Some(offset);
            self.scroll_y = px(metrics.scroll_from_pointer(y, offset));
            code.reveal = false;
        }
        cx.notify();
        true
    }
    pub(super) fn code_scrollbar_move(
        &mut self,
        e: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(code) = &mut self.code else {
            return false;
        };
        let Some(rail) = code.rail else {
            return false;
        };
        let hovered = rail.contains(&e.position);
        if code.hovered != hovered {
            code.hovered = hovered;
            cx.notify();
        }
        let marker = code
            .markers
            .iter()
            .rposition(|m| m.bounds.contains(&e.position) && !m.message.is_empty());
        if code.hovered_marker != marker {
            code.hovered_marker = marker;
            cx.notify();
        }
        if let Some(offset) = code.drag_offset {
            if e.pressed_button != Some(MouseButton::Left) {
                code.drag_offset = None;
                return false;
            }
            let metrics = Metrics::new(
                self.scroll_y.as_f32(),
                code.geometry.total(),
                rail.size.height.as_f32(),
                rail.size.height.as_f32(),
            );
            self.scroll_y =
                px(metrics.scroll_from_pointer((e.position.y - rail.top()).as_f32(), offset));
            code.reveal = false;
            cx.notify();
            return true;
        }
        false
    }
    pub(super) fn code_scrollbar_up(&mut self) {
        if let Some(code) = &mut self.code {
            code.drag_offset = None;
        }
    }
    pub(super) fn code_tooltip(&self) -> Option<AnyElement> {
        let code = self.code.as_ref()?;
        let marker = code.markers.get(code.hovered_marker?)?;
        let rail = code.rail?;
        Some(
            div()
                .absolute()
                .right(px(WIDTH + 6.))
                .top(
                    (marker.bounds.top() - rail.top())
                        .min((rail.size.height - px(32.)).max(px(0.))),
                )
                .max_w(px(360.))
                .px(px(9.))
                .py_1()
                .rounded(px(vorkspace_ui::fluid::TIP_RADIUS))
                .bg(vorkspace_ui::fluid::tip_fill())
                .text_size(px(12.))
                .text_color(vorkspace_ui::fluid::tip_text(1.))
                .child(marker.message.clone())
                .into_any_element(),
        )
    }
}

// GPUI resolves an index at a soft boundary to the end of the previous row.
// Keep the clicked/navigation side of that boundary for the caret and IME.
fn line_position(line: &WrappedLine, at: usize, lh: Pixels, upstream: bool) -> Point<Pixels> {
    if !upstream {
        for (row, boundary) in line.wrap_boundaries().iter().enumerate() {
            let glyph = &line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix];
            if glyph.index == at {
                return point(px(0.), lh * (row + 1));
            }
        }
    }
    line.position_for_index(at, lh).unwrap_or_default()
}

fn paint_rail(
    i: &Input,
    rail: Bounds<Pixels>,
    scroll: Pixels,
    w: &mut Window,
    cx: &App,
) -> Vec<MarkerHit> {
    let code = i.code.as_ref().unwrap();
    let height = rail.size.height.as_f32();
    let total = code.geometry.total().max(1.);
    let metrics = Metrics::new(scroll.as_f32(), total, height, height);
    let mut markers = Vec::new();
    w.with_content_mask(Some(ContentMask { bounds: rail }), |w| {
        w.paint_quad(fill(
            Bounds::new(rail.origin, size(px(1.), rail.size.height)),
            vorkspace_ui::theme::content(0.07),
        ));
        if metrics.scrollable {
            w.paint_quad(
                fill(
                    Bounds::new(
                        rail.origin + point(px(3.), px(metrics.thumb_top)),
                        size(px(WIDTH - 6.), px(metrics.thumb_height)),
                    ),
                    vorkspace_ui::theme::content(if code.hovered || code.drag_offset.is_some() {
                        0.38 * 0.88
                    } else {
                        0.30 * 0.72
                    }),
                )
                .corner_radii(px(2.)),
            );
        }
        // Exactly one diagnostic per logical line, strongest severity wins.
        let mut diagnostics = std::collections::BTreeMap::new();
        for diagnostic in &code.analysis.diagnostics {
            let row = code.geometry.row(diagnostic.from);
            let current = diagnostics.entry(row).or_insert(diagnostic);
            if diagnostic.priority() > current.priority() {
                *current = diagnostic;
            }
        }
        for (row, diagnostic) in diagnostics {
            let bounds = Bounds::new(
                rail.origin
                    + point(
                        px(3.),
                        px(row as f32 / code.geometry.offsets.len().max(1) as f32 * height),
                    ),
                size(px(WIDTH - 5.), px(3.)),
            );
            let color = match diagnostic.severity.as_str() {
                "error" => 0xf87171,
                "warning" => 0xfbbf24,
                "info" => 0x60a5fa,
                _ => 0x969696,
            };
            let ink = if diagnostic.severity == "hint" {
                vorkspace_ui::theme::content(0.48)
            } else {
                rgb(color).into()
            };
            w.paint_quad(fill(bounds, ink).corner_radii(px(1.)));
            markers.push(MarkerHit {
                bounds,
                pos: diagnostic.from,
                diagnostic: true,
                message: diagnostic.message.clone(),
            });
        }
        let cursor_y = code.geometry.row(i.cursor()) as f32
            / code.geometry.offsets.len().max(1) as f32
            * height;
        let mut cursor_color = vorkspace_ui::theme::accent_color();
        cursor_color.a *= if i.focus.is_focused(w) { 1. } else { 0.6 };
        w.paint_quad(fill(
            Bounds::new(
                rail.origin + point(px(0.), px(cursor_y)),
                size(rail.size.width, px(2.)),
            ),
            cursor_color,
        ));
        let (add, del) = vorkspace_ui::editor_scrollbar::diff_colors(cx);
        // Git's layer is above the cursor/diagnostic layer in editorGit.ts.
        for change in &code.analysis.changes {
            let row = code.geometry.row(change.from);
            let top = code.geometry.top(row, i.line_height);
            let bottom = if change.kind == "del" {
                top + change.deleted.max(1) as f32 * i.line_height
            } else {
                code.geometry.top(
                    code.geometry.row(change.to.saturating_sub(1)) + 1,
                    i.line_height,
                )
            };
            // A slim lane at the rail's outer edge: in a short file a change
            // spans most of the rail, and a full-width band reads as a slab.
            let bounds = Bounds::new(
                rail.origin + point(px(WIDTH - 6.), px(top / total * height)),
                size(
                    px(4.),
                    px(((bottom - top).max(0.) / total * height).max(3.)),
                ),
            );
            if change.kind == "mod" {
                let half = bounds.size.width / 2.;
                w.paint_quad(
                    fill(
                        Bounds::new(bounds.origin, size(half, bounds.size.height)),
                        rgb(del),
                    )
                    .corner_radii(px(1.)),
                );
                w.paint_quad(
                    fill(
                        Bounds::new(
                            bounds.origin + point(half, px(0.)),
                            size(half, bounds.size.height),
                        ),
                        rgb(add),
                    )
                    .corner_radii(px(1.)),
                );
            } else {
                w.paint_quad(
                    fill(bounds, rgb(if change.kind == "del" { del } else { add }))
                        .corner_radii(px(1.)),
                );
            }
            markers.push(MarkerHit {
                bounds,
                pos: change.from,
                diagnostic: false,
                message: String::new(),
            });
        }
    });
    markers
}

pub(super) struct CodeElement {
    pub input: Entity<Input>,
}
pub(super) struct CodePaint {
    lines: Vec<(usize, WrappedLine)>,
    scroll: Point<Pixels>,
    text_bounds: Bounds<Pixels>,
    text_hitbox: gpui::Hitbox,
}
impl IntoElement for CodeElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
fn shape(i: &Input, row: usize, width: Option<Pixels>, w: &mut Window) -> WrappedLine {
    let code = i.code.as_ref().unwrap();
    let start = code.geometry.offsets[row];
    let end = code.geometry.source_end(row, i.content.len());
    let text: SharedString = if let Some(fold) = code.geometry.fold_at(row) {
        format!(
            "{}…{}",
            &i.content[start..fold.from],
            &i.content[fold.to..end]
        )
        .into()
    } else {
        i.content[start..end].to_string().into()
    };
    let base = TextRun {
        len: text.len(),
        font: gpui::font(vorkspace_ui::fonts::FIXED_FONT),
        color: vorkspace_ui::theme::content(1.),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::new();
    let tokens = &code.analysis.tokens;
    let first = tokens.partition_point(|t| t.to <= start);
    let mut spans = Vec::new();
    for token in tokens.iter().skip(first).take_while(|t| t.from < end) {
        let from = token.from.max(start);
        let to = token.to.min(end);
        let color = vorkspace_ui::theme::syntax_color(token.kind.as_ref());
        if let Some(fold) = code.geometry.fold_at(row) {
            if from < fold.from {
                spans.push((from - start, to.min(fold.from) - start, color));
            }
            if to > fold.to {
                spans.push((
                    code.geometry.display_offset(row, from.max(fold.to)),
                    code.geometry.display_offset(row, to),
                    color,
                ));
            }
        } else {
            spans.push((from - start, to - start, color));
        }
    }
    if let Some(fold) = code.geometry.fold_at(row) {
        spans.push((
            fold.from - start,
            fold.from - start + "…".len(),
            vorkspace_ui::theme::content(0.4),
        ));
    }
    spans.sort_by_key(|s| s.0);
    let mut at = 0;
    for (from, to, color) in spans {
        if from > at {
            runs.push(TextRun {
                len: from - at,
                ..base.clone()
            });
        }
        if to > from {
            runs.push(TextRun {
                len: to - from,
                color,
                ..base.clone()
            });
        }
        at = to;
    }
    if at < text.len() || runs.is_empty() {
        runs.push(TextRun {
            len: text.len() - at,
            ..base
        });
    }
    if let Some(marked) = &i.marked {
        let marked = marked.start.saturating_sub(start).min(text.len())
            ..marked.end.saturating_sub(start).min(text.len());
        if !marked.is_empty() {
            let mut at = 0;
            let mut decorated = Vec::new();
            for run in runs {
                let end = at + run.len;
                let a = marked.start.clamp(at, end);
                let b = marked.end.clamp(at, end);
                if at < a {
                    decorated.push(TextRun {
                        len: a - at,
                        ..run.clone()
                    });
                }
                if a < b {
                    decorated.push(TextRun {
                        len: b - a,
                        underline: Some(UnderlineStyle {
                            color: Some(run.color),
                            thickness: px(1.),
                            wavy: false,
                        }),
                        ..run.clone()
                    });
                }
                if b < end {
                    decorated.push(TextRun {
                        len: end - b,
                        ..run
                    });
                }
                at = end;
            }
            runs = decorated;
        }
    }
    for diagnostic in code
        .analysis
        .diagnostics
        .iter()
        .filter(|_| code.geometry.fold_at(row).is_none())
    {
        let range = diagnostic.range();
        if range.end <= start || range.start >= end {
            continue;
        }
        let from = range.start.saturating_sub(start).min(text.len());
        let to = range.end.saturating_sub(start).min(text.len());
        let mut at = 0;
        let mut decorated = Vec::new();
        for run in runs {
            let end = at + run.len;
            let a = from.clamp(at, end);
            let b = to.clamp(at, end);
            if at < a {
                decorated.push(TextRun {
                    len: a - at,
                    ..run.clone()
                });
            }
            if a < b {
                decorated.push(TextRun {
                    len: b - a,
                    underline: Some(UnderlineStyle {
                        color: Some(vorkspace_ui::theme::danger()),
                        thickness: px(1.),
                        wavy: true,
                    }),
                    ..run.clone()
                });
            }
            if b < end {
                decorated.push(TextRun {
                    len: end - b,
                    ..run
                });
            }
            at = end;
        }
        runs = decorated;
    }
    w.text_system()
        .shape_text(text, px(i.font_size), &runs, width, None)
        .unwrap()
        .remove(0)
}
impl Element for CodeElement {
    type RequestLayoutState = ();
    type PrepaintState = CodePaint;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        w: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (w.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) -> CodePaint {
        // Measure wrapped block heights only when the document or pane width
        // changes. Short lines use a conservative UTF-8 byte-width bound;
        // offscreen lines do not retain glyph runs.
        self.input.update(cx, |i, _| {
            let code = i.code.as_ref().unwrap();
            let gutter = px(code.geometry.gutter());
            let width = (bounds.size.width - gutter - px(WIDTH + 12.)).max(px(1.));
            if code.layout_width != Some(width) {
                let mut tops = Vec::with_capacity(code.geometry.offsets.len() + 1);
                let mut y = TOP_PADDING;
                let sample = w.text_system().shape_line(
                    "M".into(),
                    px(i.font_size),
                    &[TextRun {
                        len: 1,
                        font: gpui::font(vorkspace_ui::fonts::FIXED_FONT),
                        color: vorkspace_ui::theme::content(1.),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                for row in 0..code.geometry.offsets.len() {
                    tops.push(y);
                    if code.geometry.hidden(row) {
                        continue;
                    }
                    let start = code.geometry.offsets[row];
                    let end = code
                        .geometry
                        .offsets
                        .get(row + 1)
                        .map_or(i.content.len(), |n| n - 1);
                    let text = &i.content[start..end];
                    let short = !text.contains('\t') && sample.width * text.len() <= width;
                    y += if !code.wrap || short {
                        i.line_height
                    } else {
                        shape(i, row, Some(width), w)
                            .size(i.line_height())
                            .height
                            .as_f32()
                    };
                }
                tops.push(y);
                let code = i.code.as_mut().unwrap();
                code.geometry.tops = Rc::new(tops);
                code.layout_width = Some(width);
            }
        });
        let i = self.input.read(cx);
        let code = i.code.as_ref().unwrap();
        let gutter = px(code.geometry.gutter());
        let text_bounds = Bounds::new(
            bounds.origin + point(gutter, px(0.)),
            size(
                (bounds.size.width - gutter - px(WIDTH + 12.)).max(px(1.)),
                bounds.size.height,
            ),
        );
        let wrap = code.wrap.then_some(text_bounds.size.width);
        let mut scroll = point(i.scroll_x, i.scroll_y);
        if code.reveal {
            let row = code.geometry.display_row(i.cursor());
            let caret_line = shape(i, row, wrap, w);
            let p = line_position(
                &caret_line,
                code.geometry.display_offset(row, i.cursor()),
                i.line_height(),
                code.affinity.get() == Some((i.cursor(), true)),
            );
            let y = px(code.geometry.top(row, i.line_height)) + p.y;
            if y < scroll.y {
                scroll.y = y;
            }
            if y + i.line_height() > scroll.y + bounds.size.height {
                scroll.y = y + i.line_height() - bounds.size.height;
            }
            if p.x < scroll.x {
                scroll.x = p.x;
            }
            if p.x + px(8.) > scroll.x + text_bounds.size.width {
                scroll.x = p.x + px(8.) - text_bounds.size.width;
            }
        }
        if code.wrap {
            scroll.x = px(0.);
        }
        scroll.y = scroll
            .y
            .max(px(0.))
            .min((px(code.geometry.total()) - bounds.size.height).max(px(0.)));
        let start = code.geometry.row_at_y(scroll.y.as_f32());
        let end = (code
            .geometry
            .row_at_y((scroll.y + bounds.size.height).as_f32())
            + 2)
        .min(code.geometry.offsets.len());
        let lines = (start..end)
            .filter(|row| !code.geometry.hidden(*row))
            .map(|row| (row, shape(i, row, wrap, w)))
            .collect();
        CodePaint {
            lines,
            scroll,
            text_bounds,
            text_hitbox: w.insert_hitbox(text_bounds, gpui::HitboxBehavior::Normal),
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut CodePaint,
        w: &mut Window,
        cx: &mut App,
    ) {
        let i = self.input.read(cx);
        let code = i.code.as_ref().unwrap();
        let lh = i.line_height();
        // The gutter and scrollbar are controls. Only the editable text viewport
        // owns the I-beam, including blank space where a click can place the caret.
        if code.drag_offset.is_none() {
            w.set_cursor_style(CursorStyle::IBeam, &state.text_hitbox);
        }
        let cursor = i.cursor();
        let selected = i.selection.clone();
        let focused = i.focus.is_focused(w);
        let line_height = i.line_height;
        let code_affinity = code.affinity.get();
        let rail = Bounds::new(
            point(bounds.right() - px(WIDTH), bounds.top()),
            size(px(WIDTH), bounds.size.height),
        );
        let markers = paint_rail(i, rail, state.scroll.y, w, cx);
        let content_len = i.content.len();
        let geometry = code.geometry.clone();
        let analysis = code.analysis.clone();
        let search_matches = code.search_matches.clone();
        w.handle_input(
            &i.focus,
            ElementInputHandler::new(state.text_bounds, self.input.clone()),
            cx,
        );
        w.with_content_mask(Some(ContentMask { bounds }), |w| {
            for (row, _) in &state.lines {
                let top = geometry.top(*row, line_height);
                if analysis.folds.iter().any(|f| geometry.row(f.from) == *row) {
                    let arrow: SharedString = if geometry.fold_at(*row).is_some() {
                        "›"
                    } else {
                        "⌄"
                    }
                    .into();
                    let arrow_line = w.text_system().shape_line(
                        arrow,
                        px(12.),
                        &[TextRun {
                            len: if geometry.fold_at(*row).is_some() {
                                "›".len()
                            } else {
                                "⌄".len()
                            },
                            font: gpui::font(vorkspace_ui::fonts::FIXED_FONT),
                            color: vorkspace_ui::theme::content(0.45),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        None,
                    );
                    let _ = arrow_line.paint(
                        point(
                            state.text_bounds.left() - px(12.),
                            bounds.top() + px(top) - state.scroll.y,
                        ),
                        lh,
                        TextAlign::Left,
                        None,
                        w,
                        cx,
                    );
                }
                let number: SharedString = (row + 1).to_string().into();
                let len = number.len();
                let line = w.text_system().shape_line(
                    number,
                    px(11.),
                    &[TextRun {
                        len,
                        font: gpui::font(vorkspace_ui::fonts::FIXED_FONT),
                        color: vorkspace_ui::theme::content(0.3),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                let _ = line.paint(
                    point(
                        state.text_bounds.left() - px(26.) - line.width,
                        bounds.top() + px(top) - state.scroll.y,
                    ),
                    lh,
                    TextAlign::Left,
                    None,
                    w,
                    cx,
                );
            }
        });
        w.with_content_mask(
            Some(ContentMask {
                bounds: state.text_bounds,
            }),
            |w| {
                for (row, line) in &state.lines {
                    let start = geometry.offsets[*row];
                    let top = geometry.top(*row, line_height);
                    let y = bounds.top() + px(top) - state.scroll.y;
                    let x = state.text_bounds.left() - state.scroll.x;
                    if cursor >= start && cursor <= geometry.source_end(*row, content_len) {
                        w.paint_quad(fill(
                            Bounds::new(
                                point(state.text_bounds.left(), y),
                                size(state.text_bounds.size.width, line.size(lh).height),
                            ),
                            vorkspace_ui::theme::content(0.1),
                        ));
                    }
                    for range in search_matches.iter() {
                        let a = geometry.display_offset(*row, range.start).min(line.len());
                        let b = geometry.display_offset(*row, range.end).min(line.len());
                        if a < b {
                            let a = line_position(line, a, lh, false);
                            let b = line.position_for_index(b, lh).unwrap_or_default();
                            if a.y == b.y {
                                w.paint_quad(fill(
                                    Bounds::new(
                                        point(x + a.x, y + a.y),
                                        size((b.x - a.x).max(px(2.)), lh),
                                    ),
                                    rgb(0xe2c08d).opacity(0.46),
                                ));
                            }
                        }
                    }
                    let a = geometry
                        .display_offset(*row, selected.start)
                        .min(line.len());
                    let b = geometry.display_offset(*row, selected.end).min(line.len());
                    if a < b
                        || (selected.start <= start + line.len()
                            && selected.end > start + line.len())
                    {
                        let a = line_position(line, a, lh, false);
                        let b = line.position_for_index(b, lh).unwrap_or_default();
                        let mut sy = a.y;
                        while sy <= b.y {
                            let left = if sy == a.y { a.x } else { px(0.) };
                            let right = if sy == b.y && selected.end <= start + line.len() {
                                b.x
                            } else {
                                state.text_bounds.size.width
                            };
                            w.paint_quad(fill(
                                Bounds::new(
                                    point(x + left, y + sy),
                                    size((right - left).max(px(2.)), lh),
                                ),
                                vorkspace_ui::theme::text_selection(),
                            ));
                            sy += lh;
                        }
                    }
                    let _ = line.paint(point(x, y), lh, TextAlign::Left, None, w, cx);
                    if focused
                        && selected.is_empty()
                        && cursor >= start
                        && cursor <= geometry.source_end(*row, content_len)
                    {
                        let p = line_position(
                            line,
                            geometry.display_offset(*row, cursor),
                            lh,
                            code_affinity == Some((cursor, true)),
                        );
                        w.paint_quad(fill(
                            Bounds::new(point(x + p.x, y + p.y), size(px(1.5), lh)),
                            vorkspace_ui::theme::content(1.),
                        ));
                    }
                }
            },
        );
        self.input.update(cx, |i, _| {
            i.bounds = Some(state.text_bounds);
            i.scroll_x = state.scroll.x;
            i.scroll_y = state.scroll.y;
            let code = i.code.as_mut().unwrap();
            code.reveal = false;
            code.rail = Some(rail);
            code.markers = markers;
            for (_, line) in &state.lines {
                code.max_width = code
                    .max_width
                    .max(line.width() - state.text_bounds.size.width + px(20.));
            }
            code.visible = std::mem::take(&mut state.lines);
        });
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod feature_tests {
    use super::*;
    use gpui::{TestAppContext, WindowBounds, WindowOptions};
    fn editor(cx: &mut TestAppContext) -> gpui::WindowHandle<Input> {
        cx.update(|cx| {
            bind(cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(500.), px(300.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(Input::editor),
            )
            .unwrap()
        })
    }
    #[gpui::test]
    fn folds_keep_source_offsets_and_expand_when_search_reveals_hidden_text(
        cx: &mut TestAppContext,
    ) {
        let window = editor(cx);
        let source = "function hello() {\n  const emoji = '🙂';\n  return emoji;\n}\n";
        window
            .update(cx, |i, _, cx| {
                i.set_text(source.into(), cx);
                i.set_editor_analysis(
                    vorkspace_editor::editor_analysis::analyze("a.js", source, None).unwrap(),
                    cx,
                );
                i.toggle_fold_row(0, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |i, _, cx| {
                let code = i.code.as_ref().unwrap();
                let fold = code.geometry.folded[0].clone();
                assert_eq!(i.content.as_ref(), source);
                assert!(
                    code.geometry.hidden(1) && code.geometry.hidden(2) && code.geometry.hidden(3)
                );
                assert_eq!(i.next(fold.from), fold.to);
                let p = i.code_position(fold.to);
                assert_eq!(i.code_index(p), fold.to);
                i.replace_bytes(0..0, "// header\n", 10, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |i, _, cx| {
                assert_eq!(
                    i.code.as_ref().unwrap().geometry.folded.len(),
                    1,
                    "editing before a fold keeps it collapsed"
                );
                let at = i.content.find("emoji").unwrap();
                i.select_range(at..at + 5, cx);
                assert!(i.code.as_ref().unwrap().geometry.folded.is_empty());
            })
            .unwrap();
    }
    #[gpui::test]
    fn autocomplete_accepts_with_tab(cx: &mut TestAppContext) {
        let window = editor(cx);
        window
            .update(cx, |i, _, cx| {
                i.set_editor_path("a.ts".into());
                i.set_text("cons".into(), cx);
                i.schedule_completion(true, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |i, w, cx| {
                let code = i.code.as_mut().unwrap();
                code.completion_index = code
                    .completions
                    .iter()
                    .position(|c| c.label == "const")
                    .unwrap();
                i.indent(&Indent, w, cx);
                assert_eq!(i.content.as_ref(), "const");
                i.undo(&Undo, w, cx);
                assert_eq!(i.content.as_ref(), "cons");
            })
            .unwrap();
    }
}
