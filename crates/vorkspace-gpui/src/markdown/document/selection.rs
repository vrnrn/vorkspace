//! Selection anchors in rendered Markdown, independent of the current layout.

use super::Document;
use crate::markdown::{self, SelectableText};
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, SharedString, TextLayout, WeakEntity, Window, div, prelude::*, px,
};
use std::{cell::RefCell, ops::Range, rc::Rc};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};
use vorkspace_ui::theme::content;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    row: usize,
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
    unit: Range<Position>,
    granularity: Granularity,
}

impl Drag {
    fn endpoints(&self, target: Range<Position>) -> (Position, Position) {
        if target.start < self.unit.start {
            (self.unit.end, target.start)
        } else {
            (self.unit.start, target.end)
        }
    }
}

struct Fragment {
    row: usize,
    offset: usize,
    len: usize,
    visible: Bounds<Pixels>,
    layout: TextLayout,
}

struct State {
    rows: Vec<SharedString>,
    fragments: Vec<Fragment>,
    endpoints: Option<(Position, Position)>,
    drag: Option<Drag>,
    dragged: bool,
}

impl State {
    fn range(&self) -> Option<Range<Position>> {
        let (anchor, head) = self.endpoints?;
        (anchor != head).then(|| anchor.min(head)..anchor.max(head))
    }

    fn unit(&self, position: Position, granularity: Granularity) -> Range<Position> {
        let text = &self.rows[position.row];
        let byte = grapheme_boundary(text, position.byte);
        let range = match granularity {
            Granularity::Character => byte..byte,
            Granularity::Word => {
                let at = if byte == text.len() {
                    text.grapheme_indices(true)
                        .next_back()
                        .map_or(0, |(ix, _)| ix)
                } else {
                    byte
                };
                text.split_word_bound_indices()
                    .find(|(ix, word)| *ix <= at && at < ix + word.len())
                    .map_or(at..at, |(ix, word)| ix..ix + word.len())
            }
            Granularity::Paragraph => {
                let start = text[..byte].rfind(['\n', '\t']).map_or(0, |ix| ix + 1);
                let end = text[byte..]
                    .find(['\n', '\t'])
                    .map_or(text.len(), |ix| byte + ix);
                start..end
            }
        };
        let at = |byte| Position { byte, ..position };
        at(range.start)..at(range.end)
    }

    fn start(&mut self, position: Position, event: &MouseDownEvent) {
        let granularity = match event.click_count {
            0 | 1 => Granularity::Character,
            2 => Granularity::Word,
            _ => Granularity::Paragraph,
        };
        let target = self.unit(position, granularity);
        let unit = match (event.modifiers.shift, self.endpoints) {
            (true, Some((anchor, _))) => anchor..anchor,
            _ => target.clone(),
        };
        let drag = Drag {
            origin: event.position,
            unit,
            granularity,
        };
        self.endpoints = Some(drag.endpoints(target));
        self.drag = Some(drag);
        self.dragged = false;
    }

    fn move_to(&mut self, point: Point<Pixels>) -> bool {
        let Some(drag) = &self.drag else {
            return false;
        };
        let delta = point - drag.origin;
        if !self.dragged && delta.x.abs().max(delta.y.abs()) < px(3.) {
            return false;
        }
        // Keep this through mouse-up so a drag returning to its start does
        // not activate a link when GPUI dispatches the click.
        self.dragged = true;
        let fragment = self.fragments.iter().min_by_key(|fragment| {
            let bounds = fragment.visible;
            let dx = (bounds.left() - point.x)
                .max(px(0.))
                .max(point.x - bounds.right());
            let dy = (bounds.top() - point.y)
                .max(px(0.))
                .max(point.y - bounds.bottom());
            (dy, dx, fragment.row, fragment.offset)
        });
        let Some(fragment) = fragment else {
            return false;
        };
        let target = self.unit(
            position_at(
                fragment.row,
                fragment.offset,
                fragment.len,
                &fragment.layout,
                point,
            ),
            drag.granularity,
        );
        let next = Some(drag.endpoints(target));
        let changed = self.endpoints != next;
        self.endpoints = next;
        changed
    }
}

fn grapheme_boundary(text: &str, byte: usize) -> usize {
    let byte = text.floor_char_boundary(byte);
    let mut cursor = GraphemeCursor::new(byte, text.len(), true);
    if cursor.is_boundary(text, 0).expect("complete text") {
        byte
    } else {
        cursor
            .prev_boundary(text, 0)
            .expect("complete text")
            .unwrap_or(0)
    }
}

fn position_at(
    row: usize,
    offset: usize,
    len: usize,
    layout: &TextLayout,
    point: Point<Pixels>,
) -> Position {
    let byte = layout.index_for_position(point).unwrap_or_else(|ix| ix);
    Position {
        row,
        byte: offset + byte.min(len),
    }
}

#[derive(Clone)]
pub(super) struct Selection(Rc<RefCell<State>>);

impl Selection {
    pub(super) fn new(rows: Vec<SharedString>) -> Self {
        Self(Rc::new(RefCell::new(State {
            rows,
            fragments: Vec::new(),
            endpoints: None,
            drag: None,
            dragged: false,
        })))
    }

    pub(super) fn begin_frame(&self) {
        self.0.borrow_mut().fragments.clear();
    }

    pub(super) fn row(&self, index: usize, view: WeakEntity<Document>) -> markdown::Selection {
        Rc::new(RowSelection {
            selection: self.clone(),
            view,
            row: index,
            offset: 0,
            cursor: CursorStyle::IBeam,
        })
    }

    pub(super) fn is_pointer_down(&self) -> bool {
        self.0.borrow().drag.is_some()
    }

    pub(super) fn move_pointer(&self, event: &MouseMoveEvent, cx: &mut Context<Document>) {
        let mut state = self.0.borrow_mut();
        if event.pressed_button != Some(MouseButton::Left) {
            state.drag = None;
        } else if state.move_to(event.position) {
            cx.notify();
        }
    }

    pub(super) fn end_pointer(&self, event: &MouseUpEvent, cx: &mut Context<Document>) {
        let mut state = self.0.borrow_mut();
        let changed = state.move_to(event.position);
        state.drag = None;
        if changed {
            cx.notify();
        }
    }

    pub(super) fn clear(&self) {
        let mut state = self.0.borrow_mut();
        state.endpoints = None;
        state.drag = None;
        state.dragged = false;
    }

    pub(super) fn select_all(&self) {
        let mut state = self.0.borrow_mut();
        state.drag = None;
        state.dragged = false;
        state.endpoints = state.rows.last().map(|last| {
            (
                Position { row: 0, byte: 0 },
                Position {
                    row: state.rows.len() - 1,
                    byte: last.len(),
                },
            )
        });
    }

    pub(super) fn copy(&self) -> Option<String> {
        let state = self.0.borrow();
        let Range { start, end } = state.range()?;
        let mut text = String::new();
        for row in start.row..=end.row {
            if row > start.row {
                text.push('\n');
            }
            let row_text = &state.rows[row];
            let from = if row == start.row { start.byte } else { 0 };
            let to = if row == end.row {
                end.byte
            } else {
                row_text.len()
            };
            text.push_str(&row_text[from..to]);
        }
        Some(text)
    }
}

#[derive(Clone)]
struct RowSelection {
    selection: Selection,
    view: WeakEntity<Document>,
    row: usize,
    offset: usize,
    cursor: CursorStyle,
}

impl SelectableText for RowSelection {
    fn with_text(&self, _: SharedString) -> markdown::Selection {
        // Document rows already provide the full text used by each fragment.
        Rc::new(self.clone())
    }

    fn with_cursor(&self, cursor: CursorStyle) -> markdown::Selection {
        Rc::new(Self {
            cursor,
            ..self.clone()
        })
    }

    fn offset(&self, offset: usize) -> markdown::Selection {
        Rc::new(Self {
            offset: self.offset + offset,
            ..self.clone()
        })
    }

    fn has_selection(&self) -> bool {
        let state = self.selection.0.borrow();
        state.dragged || state.range().is_some()
    }

    fn wrap(&self, text: SharedString, layout: TextLayout, child: AnyElement) -> AnyElement {
        let context = self.clone();
        let click_layout = layout.clone();
        let len = text.len();
        div()
            .relative()
            .cursor(self.cursor)
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let _ = context.view.update(cx, |document, cx| {
                    if !Rc::ptr_eq(&context.selection.0, &document.selection.0) {
                        return;
                    }
                    let at = position_at(
                        context.row,
                        context.offset,
                        len,
                        &click_layout,
                        event.position,
                    );
                    context.selection.0.borrow_mut().start(at, event);
                    window.focus(&document.focus, cx);
                    cx.notify();
                });
            })
            .child(SelectedText {
                context: self.clone(),
                len,
                layout,
                child,
            })
            .into_any_element()
    }
}

struct SelectedText {
    context: RowSelection,
    len: usize,
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
        let visible = self
            .layout
            .bounds()
            .intersect(&window.content_mask().bounds);
        if visible.size.width > px(0.) && visible.size.height > px(0.) {
            self.context
                .selection
                .0
                .borrow_mut()
                .fragments
                .push(Fragment {
                    row: self.context.row,
                    offset: self.context.offset,
                    len: self.len,
                    visible,
                    layout: self.layout.clone(),
                });
        }
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
        let state = self.context.selection.0.borrow();
        if let Some(Range { start, end }) = state.range()
            && (start.row..=end.row).contains(&self.context.row)
        {
            let local_byte = |byte: usize| byte.saturating_sub(self.context.offset).min(self.len);
            let from = if start.row == self.context.row {
                local_byte(start.byte)
            } else {
                0
            };
            let to = if end.row == self.context.row {
                local_byte(end.byte)
            } else {
                self.len
            };
            if from < to {
                self.layout.for_each_bounds_for_range(from..to, |bounds| {
                    window.paint_quad(gpui::fill(bounds, content(0.2)));
                });
            }
        }
        drop(state);
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{HeadlessAppContext, InputEvent, Keystroke, point, size};
    use std::sync::Arc;

    #[test]
    fn drag_across_prose_code_and_table_copies_the_painted_range() {
        const SOURCE: &str = "# Séance\n\nAvant e\u{301}clair après.\n\n```text\n    café\n  茶\n```\n\n| Nom | Valeur |\n| --- | ---: |\n| Crème | élève |\n\nSuite.";
        const PROSE: &str = "Avant e\u{301}clair après.";
        const CODE: &str = "    café";
        const CELL: &str = "élève";
        const PARTIAL: &str = "e\u{301}clair après.\n    café\n  茶\nNom\tValeur\nCrème\té";
        const ALL: &str =
            "Séance\nAvant e\u{301}clair après.\n    café\n  茶\nNom\tValeur\nCrème\télève\nSuite.";

        let mut cx = HeadlessAppContext::with_asset_source(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(crate::assets::Assets),
        );
        let window = cx
            .open_window(size(px(500.), px(480.)), |window, cx| {
                window.set_ui_scale(1.1, cx);
                cx.new(|cx| Document::meeting(SOURCE, cx))
            })
            .unwrap();
        let draw = |cx: &mut HeadlessAppContext| {
            cx.run_until_parked();
            // Drawing and dispatching can update the Document window root.
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
        };
        let send = |cx: &mut HeadlessAppContext, event| {
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .unwrap();
            draw(cx);
        };
        let copy = |cx: &mut HeadlessAppContext| {
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_action(Box::new(crate::input::Copy), cx);
            })
            .unwrap();
            cx.update(|cx| cx.read_from_clipboard().unwrap().text().unwrap())
        };
        let glyph = |cx: &mut HeadlessAppContext, text: &str, byte| {
            window
                .update(cx, |document, _, _| {
                    let state = document.selection.0.borrow();
                    let fragment = state
                        .fragments
                        .iter()
                        .find(|f| state.rows[f.row].get(f.offset..f.offset + f.len) == Some(text))
                        .unwrap_or_else(|| panic!("missing laid-out fragment {text:?}"));
                    let at = fragment.layout.position_for_index(byte).unwrap();
                    at + point(px(1.), fragment.layout.line_height() / 2.)
                })
                .unwrap()
        };
        let painted = |cx: &mut HeadlessAppContext, point: Point<Pixels>| {
            window
                .update(cx, |_, window, _| {
                    let x = point.x.as_f32() * window.scale_factor();
                    let y = point.y.as_f32() * window.scale_factor();
                    window.painted_quads().iter().any(|quad| {
                        let contains = |bounds: &Bounds<gpui::ScaledPixels>| {
                            bounds.left().0 <= x
                                && x < bounds.right().0
                                && bounds.top().0 <= y
                                && y < bounds.bottom().0
                        };
                        quad.background == gpui::Background::from(content(0.2))
                            && contains(&quad.bounds)
                            && contains(&quad.content_mask.bounds)
                    })
                })
                .unwrap()
        };
        draw(&mut cx);
        let a = glyph(&mut cx, PROSE, "Avant ".len()).map(|p| p * 1.1);
        let b = glyph(&mut cx, CELL, "é".len()).map(|p| p * 1.1);

        for (from, to) in [(a, b), (b, a)] {
            send(
                &mut cx,
                MouseMoveEvent {
                    position: from,
                    ..Default::default()
                }
                .to_platform_input(),
            );
            send(
                &mut cx,
                MouseDownEvent {
                    button: MouseButton::Left,
                    position: from,
                    click_count: 1,
                    ..Default::default()
                }
                .to_platform_input(),
            );
            send(
                &mut cx,
                MouseMoveEvent {
                    position: to,
                    pressed_button: Some(MouseButton::Left),
                    ..Default::default()
                }
                .to_platform_input(),
            );
            send(
                &mut cx,
                MouseUpEvent {
                    button: MouseButton::Left,
                    position: to,
                    click_count: 1,
                    ..Default::default()
                }
                .to_platform_input(),
            );
            assert_eq!(copy(&mut cx), PARTIAL);
            let code_glyph = glyph(&mut cx, CODE, "    ca".len());
            assert!(painted(&mut cx, code_glyph));
            let selected_cell_glyph = glyph(&mut cx, CELL, 0);
            let unselected_cell_glyph = glyph(&mut cx, CELL, "é".len());
            assert!(painted(&mut cx, selected_cell_glyph));
            assert!(!painted(&mut cx, unselected_cell_glyph));
        }
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse("cmd-a").unwrap(), cx);
        })
        .unwrap();
        draw(&mut cx);
        assert_eq!(copy(&mut cx), ALL);
    }

    #[test]
    fn byte_anchors_stay_on_extended_grapheme_boundaries() {
        let text = "aé👩‍💻e\u{301}z";
        for (start, grapheme) in text.grapheme_indices(true) {
            for byte in start..start + grapheme.len() {
                assert_eq!(grapheme_boundary(text, byte), start);
            }
        }
        assert_eq!(grapheme_boundary(text, text.len() + 1), text.len());
    }
}
