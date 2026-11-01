//! Select the rendered text, retaining byte anchors instead of shaped history.
use super::*;
use gpui::{
    AnyElement, App, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId,
    MouseButton, Point, TextLayout, WeakEntity,
};
use std::{cell::RefCell, ops::Range, rc::Rc};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    row: (usize, usize),
    byte: usize,
}
#[derive(Clone, Copy)]
enum Granularity {
    Character,
    Word,
    Paragraph,
}
struct Drag {
    origin: Point<Pixels>,
    anchor: Range<Position>,
    granularity: Granularity,
    moved: bool,
}
#[derive(Default)]
struct State {
    anchor: Option<Position>,
    head: Option<Position>,
    response: usize,
    quote: bool,
    drag: Option<Drag>,
    menu: Option<Point<Pixels>>,
    layouts: HashMap<((usize, usize), usize, usize), Fragment>,
    reader: Option<ReaderAnchor>,
}
#[derive(Clone)]
struct ReaderAnchor {
    position: Position,
    text: SharedString,
    top: Pixels,
    scroll: ListOffset,
}
struct Fragment {
    row: (usize, usize),
    response: usize,
    offset: usize,
    text: SharedString,
    row_text: SharedString,
    layout: TextLayout,
}
#[derive(Clone, Default)]
pub(super) struct Selection(Rc<RefCell<State>>);
#[derive(Clone)]
pub(crate) struct TextSelection {
    state: Selection,
    view: WeakEntity<TranscriptView>,
    row: (usize, usize),
    response: usize,
    offset: usize,
    quote: bool,
    row_text: Option<SharedString>,
    cursor: gpui::CursorStyle,
}
impl Selection {
    /// A prepended history page moves local source indices but not the text
    /// the user selected. Layout fragments are rebuilt in the next frame.
    pub(super) fn shift_source(&self, amount: usize) {
        let mut state = self.0.borrow_mut();
        let shift =
            |position: &mut Position| position.row.0 = position.row.0.saturating_add(amount);
        if let Some(position) = &mut state.anchor {
            shift(position);
        }
        if let Some(position) = &mut state.head {
            shift(position);
        }
        if let Some(drag) = &mut state.drag {
            shift(&mut drag.anchor.start);
            shift(&mut drag.anchor.end);
        }
        state.response = state.response.saturating_add(amount);
        state.layouts.clear();
        state.reader = None;
    }
    pub(super) fn hide_menu(&self) {
        let mut state = self.0.borrow_mut();
        state.menu = None;
        state.drag = None;
    }
    pub(super) fn clear_if_not_dragging(&self) {
        let dragging = self.0.borrow().drag.is_some();
        if !dragging {
            self.clear();
        }
    }
    pub(super) fn active(&self) -> bool {
        let state = self.0.borrow();
        state.drag.is_some() || state.menu.is_some() || state.anchor != state.head
    }
    pub(super) fn hide(&self) {
        let mut state = self.0.borrow_mut();
        state.anchor = None;
        state.head = None;
        state.drag = None;
        state.menu = None;
        state.layouts.clear();
    }
    pub(super) fn clear(&self) {
        self.hide();
        self.forget_reader();
    }
    pub(super) fn retain_source_prefix(&self, end: usize) {
        let stable = {
            let state = self.0.borrow();
            state.response < end
                && [state.anchor, state.head]
                    .into_iter()
                    .flatten()
                    .all(|position| position.row.0 < end)
        };
        if !stable {
            self.clear();
        } else {
            self.begin_frame();
        }
    }
    pub(super) fn begin_frame(&self) {
        self.0.borrow_mut().layouts.clear();
    }
    pub(super) fn forget_reader(&self) {
        self.0.borrow_mut().reader = None;
    }
    pub(super) fn remember_reader(&self, viewport: Bounds<Pixels>, list: &ListState) {
        let mut state = self.0.borrow_mut();
        state.reader = if list.is_following_tail() {
            None
        } else if let Some(anchor) = state.reader.as_ref()
            && anchor_y(&state, anchor)
                .is_some_and(|y| (y - viewport.top() - anchor.top).abs() <= px(1.))
        {
            // Keep the same glyph throughout a divider drag. Choosing a new
            // word after each width step would slowly drift by wrapped lines.
            let mut anchor = anchor.clone();
            anchor.scroll = list.logical_scroll_top();
            Some(anchor)
        } else {
            reading_anchor(&state, viewport, list.logical_scroll_top())
        };
    }
    fn range(
        &self,
        row: (usize, usize),
        response: usize,
        offset: usize,
        len: usize,
    ) -> Range<usize> {
        let s = self.0.borrow();
        let (Some(a), Some(b)) = (s.anchor, s.head) else {
            return 0..0;
        };
        if response != s.response {
            return 0..0;
        }
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        if row < a.row || row > b.row {
            return 0..0;
        }
        let start = if row == a.row {
            a.byte.saturating_sub(offset).min(len)
        } else {
            0
        };
        let end = if row == b.row {
            b.byte.saturating_sub(offset).min(len)
        } else {
            len
        };
        start.min(end)..end
    }
}

fn anchor_y(state: &State, anchor: &ReaderAnchor) -> Option<Pixels> {
    let byte = anchor.position.byte;
    let end = byte + anchor.text.get(byte..)?.chars().next()?.len_utf8();
    state.layouts.values().find_map(|fragment| {
        (fragment.row == anchor.position.row
            && fragment.offset <= byte
            && byte < fragment.offset + fragment.text.len()
            && ((fragment.row_text.len() == anchor.text.len()
                && fragment.row_text.as_ptr() == anchor.text.as_ptr())
                || fragment.row_text.get(..end) == anchor.text.get(..end)))
        .then(|| {
            fragment
                .layout
                .position_for_index(byte - fragment.offset)
                .map(|p| p.y)
        })
        .flatten()
    })
}

fn reading_anchor(
    state: &State,
    viewport: Bounds<Pixels>,
    scroll: ListOffset,
) -> Option<ReaderAnchor> {
    for y in [32., 48., 64.] {
        for x in [0.25, 0.5, 0.75] {
            let point = viewport.origin + gpui::point(viewport.size.width * x, px(y));
            for fragment in state.layouts.values() {
                if !fragment.layout.bounds().contains(&point) {
                    continue;
                }
                let byte = snap(
                    &fragment.text,
                    fragment
                        .layout
                        .index_for_position(point)
                        .unwrap_or_else(|ix| ix),
                );
                if fragment.text[byte..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
                {
                    continue;
                }
                let top = fragment.layout.position_for_index(byte)?.y - viewport.top();
                return Some(ReaderAnchor {
                    position: Position {
                        row: fragment.row,
                        byte: fragment.offset + byte,
                    },
                    text: fragment.row_text.clone(),
                    top,
                    scroll,
                });
            }
        }
    }
    None
}

impl TranscriptView {
    /// Rebase a paused reader before the visible list prepaints at a new width.
    /// Only the bookmarked text row is probed; GPUI discards its hitboxes and
    /// event handlers so the correction cannot expose an intermediate frame.
    pub(super) fn restore_reading_glyph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.list.is_following_tail() || self.list.is_scrollbar_dragging() {
            return;
        }
        let Some(anchor) = self.selection.0.borrow().reader.clone() else {
            return;
        };
        let scroll = self.list.logical_scroll_top();
        if scroll.item_ix != anchor.scroll.item_ix
            || scroll.offset_in_item != anchor.scroll.offset_in_item
        {
            return;
        }
        let Some(ix) = self.rows.iter().position(|r| r.key == anchor.position.row) else {
            return;
        };
        if !matches!(self.rows[ix].body, Body::Plain(_) | Body::Markdown(_))
            || self.rows[ix].in_phase()
            || self.motions.contains_key(&anchor.position.row)
            || !self.messages[self.rows[ix].message].attachments.is_empty()
        {
            return;
        }
        let mut row = self.row(ix, window, cx);
        row.layout_as_root(
            size(
                AvailableSpace::Definite(self.viewport.size.width),
                AvailableSpace::MinContent,
            ),
            window,
            cx,
        );
        let probe: Result<(), Option<Pixels>> = window.transact(|window| {
            row.prepaint_at(gpui::point(px(0.), px(0.)), window, cx);
            let state = self.selection.0.borrow();
            let y = anchor_y(&state, &anchor);
            Err(y)
        });
        self.selection.begin_frame();
        if let Err(Some(y)) = probe {
            let mut item_ix = ix;
            let mut offset_in_item = y + px(TOP_INSET) - anchor.top;
            while offset_in_item < px(0.) && item_ix > 0 {
                item_ix -= 1;
                offset_in_item += self.measure_row_height(item_ix, window, cx);
            }
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: offset_in_item.max(px(0.)),
            });
            // The byte anchor supplies the final position; animating the old
            // row coordinate would move that glyph away again after painting.
            self.reflow = Default::default();
        }
    }
}
const MENU_DELAY: std::time::Duration = std::time::Duration::from_millis(300);
/// The top centre of the selection's first line, as React anchors to the
/// range's first client rect rather than the pointer.
fn menu_anchor(state: &State) -> Option<Point<Pixels>> {
    let (a, b) = (state.anchor?, state.head?);
    let (start, end) = if a < b { (a, b) } else { (b, a) };
    let fragment = state.layouts.values().find(|f| {
        f.response == state.response
            && f.row == start.row
            && f.offset <= start.byte
            && start.byte < f.offset + f.text.len()
    })?;
    let top = fragment
        .layout
        .position_for_index(start.byte - fragment.offset)?;
    let right = (end.row == start.row && end.byte <= fragment.offset + fragment.text.len())
        .then(|| {
            fragment
                .layout
                .position_for_index(end.byte - fragment.offset)
        })
        .flatten()
        .filter(|p| p.y == top.y)
        .map_or(fragment.layout.bounds().right(), |p| p.x);
    Some(gpui::point((top.x + right) / 2., top.y))
}
fn snap(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    text.grapheme_indices(true)
        .map(|(ix, _)| ix)
        .take_while(|ix| *ix <= byte)
        .last()
        .unwrap_or(0)
}
fn word(text: &str, byte: usize) -> Range<usize> {
    let at = if byte >= text.len() {
        text.grapheme_indices(true)
            .next_back()
            .map_or(0, |(ix, _)| ix)
    } else {
        snap(text, byte)
    };
    text.split_word_bound_indices()
        .find(|(ix, s)| *ix <= at && at < *ix + s.len())
        .map(|(ix, s)| ix..ix + s.len())
        .unwrap_or(at..at)
}
fn unit(text: &str, byte: usize, granularity: Granularity) -> Range<usize> {
    let byte = snap(text, byte);
    match granularity {
        Granularity::Character => byte..byte,
        Granularity::Word => word(text, byte),
        Granularity::Paragraph => {
            let start = text[..byte].rfind(['\n', '\t']).map_or(0, |ix| ix + 1);
            let end = text[byte..]
                .find(['\n', '\t'])
                .map_or(text.len(), |ix| byte + ix);
            start..end
        }
    }
}
impl crate::markdown::SelectableText for TextSelection {
    fn with_text(&self, text: SharedString) -> crate::markdown::Selection {
        Rc::new(self.clone().with_text(text))
    }
    fn with_cursor(&self, cursor: gpui::CursorStyle) -> crate::markdown::Selection {
        Rc::new(self.clone().with_cursor(cursor))
    }
    fn offset(&self, offset: usize) -> crate::markdown::Selection {
        Rc::new(TextSelection::offset(self, offset))
    }
    fn has_selection(&self) -> bool {
        TextSelection::has_selection(self)
    }
    fn wrap(&self, text: SharedString, layout: TextLayout, child: AnyElement) -> AnyElement {
        TextSelection::wrap(self, text, layout, child)
    }
}
impl TextSelection {
    pub(crate) fn with_text(mut self, text: SharedString) -> Self {
        self.row_text = Some(text);
        self
    }
    pub(crate) fn with_cursor(mut self, cursor: gpui::CursorStyle) -> Self {
        self.cursor = cursor;
        self
    }
    pub(crate) fn offset(&self, offset: usize) -> Self {
        let mut next = self.clone();
        next.offset += offset;
        next
    }
    pub(crate) fn has_selection(&self) -> bool {
        let state = self.state.0.borrow();
        state.anchor != state.head || state.drag.as_ref().is_some_and(|drag| drag.moved)
    }
    pub(crate) fn wrap(
        &self,
        text: SharedString,
        layout: TextLayout,
        child: AnyElement,
    ) -> AnyElement {
        let selection = self.clone();
        let click_layout = layout.clone();
        let click_text = text.clone();
        let row_text = self.row_text.clone().unwrap_or_else(|| text.clone());
        div()
            .relative()
            .cursor(self.cursor)
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let byte = click_layout
                    .index_for_position(event.position)
                    .unwrap_or_else(|ix| ix);
                let byte = snap(&click_text, byte);
                let granularity = if event.click_count >= 3 {
                    Granularity::Paragraph
                } else if event.click_count == 2 {
                    Granularity::Word
                } else {
                    Granularity::Character
                };
                let range = unit(&row_text, selection.offset + byte, granularity);
                let _ = selection.view.update(cx, |view, cx| {
                    let mut state = selection.state.0.borrow_mut();
                    let start = Position {
                        row: selection.row,
                        byte: range.start,
                    };
                    let end = Position {
                        row: selection.row,
                        byte: range.end,
                    };
                    let extend = event.modifiers.shift
                        && state.anchor.is_some()
                        && state.response == selection.response;
                    let anchor = if extend {
                        let anchor = state.anchor.unwrap();
                        anchor..anchor
                    } else {
                        start..end
                    };
                    state.anchor = Some(anchor.start);
                    state.head = Some(if start < anchor.start { start } else { end });
                    state.response = selection.response;
                    state.quote = selection.quote;
                    state.drag = Some(Drag {
                        origin: event.position,
                        anchor,
                        granularity,
                        moved: false,
                    });
                    state.menu = None;
                    drop(state);
                    if let Some(focus) = &view.selection_focus {
                        window.focus(focus, cx)
                    }
                    cx.notify();
                });
            })
            .child(SelectedText {
                context: self.clone(),
                text,
                layout,
                child,
            })
            .into_any_element()
    }
}
struct SelectedText {
    context: TextSelection,
    text: SharedString,
    layout: TextLayout,
    child: AnyElement,
}
impl IntoElement for SelectedText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for SelectedText {
    type RequestLayoutState = ();
    type PrepaintState = ();
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
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
        self.context.state.0.borrow_mut().layouts.insert(
            (self.context.row, self.context.response, self.context.offset),
            Fragment {
                row: self.context.row,
                response: self.context.response,
                offset: self.context.offset,
                text: self.text.clone(),
                row_text: self
                    .context
                    .row_text
                    .clone()
                    .unwrap_or_else(|| self.text.clone()),
                layout: self.layout.clone(),
            },
        );
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let selected = self.context.state.range(
            self.context.row,
            self.context.response,
            self.context.offset,
            self.text.len(),
        );
        if !selected.is_empty() {
            let bounds = self.layout.bounds();
            let height = self.layout.line_height();
            if let (Some(a), Some(b)) = (
                self.layout.position_for_index(selected.start),
                self.layout.position_for_index(selected.end),
            ) {
                let mut y = a.y;
                while y <= b.y {
                    let left = if y == a.y { a.x } else { bounds.left() };
                    let right = if y == b.y { b.x } else { bounds.right() };
                    if right > left {
                        window.paint_quad(gpui::fill(
                            Bounds::new(gpui::point(left, y), size(right - left, height)),
                            content(0.2),
                        ))
                    }
                    y += height;
                }
            }
        }
        self.child.paint(window, cx);
    }
}
impl TranscriptView {
    pub(super) fn selection_turn(&self) -> Option<usize> {
        let state = self.selection.0.borrow();
        state.menu?;
        self.messages
            .iter()
            .find(|m| m.start == state.response)
            .map(|m| m.turn)
    }
    pub(super) fn text_selection(
        &self,
        row: (usize, usize),
        response: usize,
        quote: bool,
        cx: &Context<Self>,
    ) -> Option<TextSelection> {
        (!self.document).then(|| TextSelection {
            state: self.selection.clone(),
            view: cx.weak_entity(),
            row,
            response,
            offset: 0,
            quote,
            row_text: None,
            cursor: gpui::CursorStyle::IBeam,
        })
    }
    pub(super) fn markdown_selection(
        &self,
        row: (usize, usize),
        response: usize,
        quote: bool,
        cx: &Context<Self>,
    ) -> Option<crate::markdown::Selection> {
        self.text_selection(row, response, quote, cx)
            .map(|selection| Rc::new(selection) as crate::markdown::Selection)
    }
    pub(super) fn move_selection(
        &mut self,
        event: &gpui::MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut state = self.selection.0.borrow_mut();
        let Some(drag) = state.drag.as_mut() else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            state.drag = None;
            return;
        }
        if !drag.moved {
            let delta = event.position - drag.origin;
            if delta.x.abs().max(delta.y.abs()) < px(3.) {
                return;
            }
            drag.moved = true;
        }
        let anchor = drag.anchor.clone();
        let granularity = drag.granularity;
        let response = state.response;
        let hit = state
            .layouts
            .values()
            .filter(|f| f.response == response)
            .min_by(|a, b| {
                let distance = |f: &Fragment| {
                    let bounds = f.layout.bounds();
                    let dx = (bounds.left() - event.position.x)
                        .max(px(0.))
                        .max(event.position.x - bounds.right());
                    let dy = (bounds.top() - event.position.y)
                        .max(px(0.))
                        .max(event.position.y - bounds.bottom());
                    f32::from(dy) * 10000. + f32::from(dx)
                };
                distance(a).total_cmp(&distance(b))
            });
        if let Some(fragment) = hit {
            let byte = fragment
                .layout
                .index_for_position(event.position)
                .unwrap_or_else(|ix| ix);
            let range = unit(&fragment.row_text, fragment.offset + byte, granularity);
            let start = Position {
                row: fragment.row,
                byte: range.start,
            };
            let end = Position {
                row: fragment.row,
                byte: range.end,
            };
            let (anchor, head) = if start < anchor.start {
                (anchor.end, start)
            } else {
                (anchor.start, end)
            };
            if state.anchor != Some(anchor) || state.head != Some(head) {
                state.anchor = Some(anchor);
                state.head = Some(head);
                drop(state);
                cx.notify()
            }
        }
    }
    pub(super) fn finish_selection(
        &mut self,
        event: &gpui::MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut state = self.selection.0.borrow_mut();
        if state.drag.take().is_none() {
            return;
        }
        state.menu = None;
        let (anchor, head) = (state.anchor, state.head);
        drop(state);
        cx.notify();
        if anchor == head {
            return;
        }
        // Wait out a quick select-and-go before offering the menu, and drop
        // it if the selection moved on in the meantime.
        let fallback = event.position;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MENU_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                let mut state = this.selection.0.borrow_mut();
                if state.drag.is_some() || state.anchor != anchor || state.head != head {
                    return;
                }
                state.menu = Some(menu_anchor(&state).unwrap_or(fallback));
                drop(state);
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn selected_text(&self) -> Option<String> {
        let state = self.selection.0.borrow();
        let (a, b) = (state.anchor?, state.head?);
        if a == b {
            return None;
        }
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        let mut parts = Vec::new();
        for row in self.rows.iter().filter(|row| {
            row.key >= a.row
                && row.key <= b.row
                && self.messages[row.message].start == state.response
        }) {
            let text = match &row.body {
                Body::Markdown(md) => crate::markdown::selection_text(md),
                Body::Plain(text) => text.to_string(),
                Body::Interjection(note) => note.text.clone(),
                _ => continue,
            };
            let start = if row.key == a.row {
                snap(&text, a.byte)
            } else {
                0
            };
            let end = if row.key == b.row {
                snap(&text, b.byte)
            } else {
                text.len()
            };
            if start <= end {
                parts.push(text[start..end].to_owned())
            }
        }
        let text = parts.join("\n").trim().to_owned();
        (!text.is_empty()).then_some(text)
    }
    pub(super) fn copy_selection(
        &mut self,
        _: &crate::input::Copy,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            cx.stop_propagation()
        }
    }
    pub(super) fn selection_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.selection.0.borrow();
        let position = state.menu?;
        let quote = state.quote;
        let response = state.response;
        drop(state);
        let text = self.selected_text()?;
        let copy = text.clone();
        let note = text.clone();
        let turn = self
            .messages
            .iter()
            .find(|m| m.start == response)
            .map_or(response, |m| m.turn);
        let button = |id: &'static str, label: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .flex()
                .items_center()
                .gap_2()
                .h(px(32.))
                .px_2p5()
                .rounded(px(8.))
                .text_size(px(13.))
                .hover(|s| s.bg(content(0.05)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(icon(glyph, 14.))
                .child(label)
        };
        let panel = div()
            .id("transcript-selection-actions")
            .role(Role::Toolbar)
            .aria_label("Selected text actions")
            .p_1()
            .min_w(px(144.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                button("selection-copy", "Copy", "copy").on_click(cx.listener(
                    move |_, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                    },
                )),
            )
            .when(quote, |d| {
                d.child(
                    button("selection-chat", "Add to chat", "comment-add").on_click(cx.listener(
                        move |this, _, window, cx| {
                            if this.bot {
                                cx.emit(BotResponseAction::Quote {
                                    text: text.clone(),
                                    window: window.window_handle(),
                                })
                            } else {
                                cx.emit(ResponseRequest::QuoteSelection { text: text.clone() })
                            }
                            this.selection.clear();
                            cx.notify();
                        },
                    )),
                )
            })
            .when(quote, |d| {
                d.child(
                    button("selection-note", "Add to notes", "response-note").on_click(
                        cx.listener(move |this, _, _, cx| {
                            if this.note_pending.insert(turn) {
                                cx.emit(ResponseRequest::SaveNote {
                                    turn,
                                    text: note.clone(),
                                    selection: true,
                                });
                                cx.notify()
                            }
                        }),
                    ),
                )
            })
            .when_some(
                self.note_feedback
                    .get(&turn)
                    .and_then(|result| result.as_ref().err()),
                |d, error| {
                    d.child(
                        div()
                            .id("selection-note-error")
                            .role(Role::Alert)
                            .max_w(px(280.))
                            .px_2p5()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(0xf87171))
                            .child(format!("Could not save note: {error}")),
                    )
                },
            );
        let weak = cx.weak_entity();
        Some(
            popover("transcript-selection", Anchor::Point(position))
                .allow_outside_press()
                .side(Side::Top)
                .align(crate::popover::Align::Center)
                .child(panel)
                .on_dismiss(move |_, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.selection.0.borrow_mut().menu = None;
                        cx.notify()
                    });
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests;
