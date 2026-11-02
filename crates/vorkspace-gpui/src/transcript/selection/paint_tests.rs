use super::*;
use gpui::{Entity, HeadlessAppContext, InputEvent, WindowHandle};
use std::sync::Arc;

const FIRST: &str = "Flash is downloaded, and all three models are implemented in a separate signed build. Unified remains the default. The Flash path follows FluidVoice's realtime implementation; TDT now produces live text using overlapping windows.";
const TABLE: &str = "Model\tBest fit\tFirst stable text in the test\nUnified English\tEnglish with punctuation\t3.5 s\nTDT v3\t25 languages with punctuation\t3.0 s\nFlash\tFast English without punctuation\t2.5 s";
const SECOND: &str = "Those times are from one public recording through the model workers; they are not typing-latency or general accuracy benchmarks. Flash also completed two warm turns across pauses, and TDT completed a French recording. Focused tests and strict lint passed.";
const THIRD: &str = "The release modifier keys error has a targeted code fix and passing native policy tests, but I cannot yet confirm it is fixed in a real field. The separate test helper reported no Accessibility trust, so no paste was attempted. Your running app and its model selection are unchanged; they do not yet contain this build.";
const LAST: &str = "Separate signed build · Detailed evidence";
const MARKDOWN: &str = "Flash is downloaded, and all three models are implemented in a separate signed build. Unified remains the default. The Flash path follows [FluidVoice's realtime implementation](https://example.invalid); TDT now produces live text using overlapping windows.\n\n| Model | Best fit | First stable text in the test |\n| --- | --- | ---: |\n| Unified English | English with punctuation | 3.5 s |\n| [TDT v3](https://example.invalid) | 25 languages with punctuation | 3.0 s |\n| Flash | Fast English without punctuation | 2.5 s |\n\nThose times are from one public recording through the model workers; they are **not typing-latency or general accuracy benchmarks**. Flash also completed two warm turns across pauses, and TDT completed a French recording. Focused tests and strict lint passed.\n\nThe release modifier keys error has a targeted code fix and passing native policy tests, but **I cannot yet confirm it is fixed in a real field**. The separate test helper reported no Accessibility trust, so no paste was attempted. Your running app and its model selection are unchanged; they do not yet contain this build.\n\n[Separate signed build](https://example.invalid) · [Detailed evidence](https://example.invalid)";

struct Surface {
    transcript: Entity<TranscriptView>,
}

impl Render for Surface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .font_family(vorkspace_ui::fonts::UI)
            .bg(rgb(0x161616))
            .child(self.transcript.clone())
    }
}

#[derive(Clone)]
struct FragmentSnapshot {
    row: (usize, usize),
    offset: usize,
    text: SharedString,
    layout: TextLayout,
}

struct Fixture {
    cx: HeadlessAppContext,
    window: WindowHandle<Surface>,
    transcript: Entity<TranscriptView>,
}

impl Fixture {
    fn new(zoom: f32) -> Self {
        Self::with_markdown(zoom, MARKDOWN)
    }

    fn with_markdown(zoom: f32, markdown: &'static str) -> Self {
        let capture = std::env::var_os("VORKSPACE_SELECTION_CAPTURE_DIR").is_some();
        let mut cx = HeadlessAppContext::with_platform(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(crate::assets::Assets),
            move || {
                if capture {
                    Ok(Some(Box::new(
                        gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
                    )))
                } else {
                    Ok(None)
                }
            },
        );
        vorkspace_ui::clock::use_test_clock(&cx);
        let height = if zoom > 1.0 { 900. } else { 700. };
        let window = cx
            .open_window(size(px(600.), px(height)), |window, cx| {
                window.set_ui_scale(zoom, cx);
                let transcript = cx.new(|cx| {
                    let mut view = TranscriptView::new();
                    view.bot_mode("selection-fixture", 0xffffff, "Bot");
                    view.install_prepared_history(
                        PreparedHistory::new(
                            vec![Row {
                                role: "assistant".into(),
                                text: markdown.into(),
                                first: true,
                                sent_at: None,
                            }],
                            true,
                        ),
                        cx,
                    );
                    view
                });
                cx.new(|_| Surface { transcript })
            })
            .unwrap();
        vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
        let transcript = window
            .update(&mut cx, |surface, _, _| surface.transcript.clone())
            .unwrap();
        transcript.update(&mut cx, |view, cx| {
            let first = view
                .rows
                .iter()
                .position(|row| matches!(&row.body, Body::Markdown(_)))
                .unwrap();
            view.list.pause_following_tail();
            view.list.scroll_to(ListOffset {
                item_ix: first,
                offset_in_item: px(0.13),
            });
            cx.notify();
        });
        let mut fixture = Self {
            cx,
            window,
            transcript,
        };
        fixture.draw();
        fixture
    }

    fn draw(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
    }

    fn fragment(&mut self, text: &str) -> FragmentSnapshot {
        self.window
            .update(&mut self.cx, |surface, _, cx| {
                let view = surface.transcript.read(cx);
                let state = view.selection.0.borrow();
                let f = state
                    .layouts
                    .values()
                    .find(|fragment| fragment.text.as_ref() == text)
                    .unwrap_or_else(|| panic!("missing laid-out fragment: {text}"));
                FragmentSnapshot {
                    row: f.row,
                    offset: f.offset,
                    text: f.text.clone(),
                    layout: f.layout.clone(),
                }
            })
            .unwrap()
    }

    fn event(&mut self, event: impl InputEvent) {
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(event.to_platform_input(), cx);
            })
            .unwrap();
        self.draw();
    }

    fn platform_point(&mut self, logical: Point<Pixels>) -> Point<Pixels> {
        self.window
            .update(&mut self.cx, |_, window, _| {
                logical.map(|coordinate| coordinate * window.ui_scale())
            })
            .unwrap()
    }

    fn drag(&mut self, from: Point<Pixels>, to: Point<Pixels>) {
        let from = self.platform_point(from);
        let to = self.platform_point(to);
        // A real pointer enters the starting glyph before its button goes down;
        // GPUI resolves its hover hitboxes on that move.
        self.event(gpui::MouseMoveEvent {
            position: from,
            ..Default::default()
        });
        self.event(gpui::MouseDownEvent {
            button: MouseButton::Left,
            position: from,
            click_count: 1,
            ..Default::default()
        });
        self.transcript.read_with(&self.cx, |view, _| {
            let state = view.selection.0.borrow();
            assert!(
                state.drag.is_some(),
                "pointer down at {from:?} did not start a selection; menu={:?}",
                state.menu
            );
        });
        self.event(gpui::MouseMoveEvent {
            position: to,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        });
        self.event(gpui::MouseUpEvent {
            button: MouseButton::Left,
            position: to,
            click_count: 1,
            ..Default::default()
        });
    }

    fn copy(&mut self) -> String {
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_action(Box::new(crate::input::Copy), cx);
            })
            .unwrap();
        self.cx.run_until_parked();
        self.cx
            .update(|cx| cx.read_from_clipboard().unwrap().text().unwrap())
    }

    fn assert_anchors(&mut self, anchor: Position, head: Position) {
        self.transcript.read_with(&self.cx, |view, _| {
            let state = view.selection.0.borrow();
            assert_eq!((state.anchor, state.head), (Some(anchor), Some(head)));
        });
    }

    fn painted_at(&mut self, point: Point<Pixels>) -> bool {
        self.window
            .update(&mut self.cx, |_, window, _| {
                let x = point.x.as_f32() * window.scale_factor();
                let y = point.y.as_f32() * window.scale_factor();
                window.painted_quads().iter().any(|quad| {
                    let contains = |bounds: &gpui::Bounds<gpui::ScaledPixels>| {
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
    }

    fn capture(&mut self, name: &str) {
        if let Some(directory) = std::env::var_os("VORKSPACE_SELECTION_CAPTURE_DIR") {
            let path = std::path::PathBuf::from(directory).join(name);
            self.cx
                .capture_screenshot(self.window.into())
                .unwrap()
                .save(path)
                .unwrap();
        }
    }
}

fn byte_point(fragment: &FragmentSnapshot, byte: usize) -> Point<Pixels> {
    let position = fragment.layout.position_for_index(byte).unwrap();
    gpui::point(
        position.x + px(1.),
        position.y + fragment.layout.line_height() / 2.,
    )
}

fn assert_glyph_coverage(
    fixture: &mut Fixture,
    fragment: &FragmentSnapshot,
    selected: Range<usize>,
) {
    let mut sampled_lines = std::collections::BTreeSet::new();
    for (byte, ch) in fragment.text.char_indices() {
        if !ch.is_ascii_graphic() {
            continue;
        }
        let Some(a) = fragment.layout.position_for_index(byte) else {
            continue;
        };
        let Some(b) = fragment.layout.position_for_index(byte + ch.len_utf8()) else {
            continue;
        };
        if a.y != b.y || b.x <= a.x {
            continue;
        }
        let expected = byte >= selected.start && byte + ch.len_utf8() <= selected.end;
        if expected {
            sampled_lines.insert((a.y.as_f32() * 100.).round() as i32);
        }
        let point = gpui::point((a.x + b.x) / 2., a.y + fragment.layout.line_height() / 2.);
        assert_eq!(
            fixture.painted_at(point),
            expected,
            "fragment {:?}, byte {byte}, char {ch:?}, point {point:?}",
            fragment.row
        );
    }
    if selected == (0..fragment.text.len()) && fragment.text.len() > 100 {
        assert!(
            sampled_lines.len() >= 3,
            "fixture paragraph must wrap across at least three lines"
        );
    }
}

#[test]
fn wrapped_markdown_drag_copy_and_paint_stay_in_sync() {
    for zoom in [0.9, 1.0, 1.1] {
        let mut fixture = Fixture::new(zoom);
        let first = fixture.fragment(FIRST);
        let last = fixture.fragment(LAST);
        let start = FIRST.find("downloaded").unwrap();
        let end = LAST.find("evidence").unwrap() + 4;
        let first_point = byte_point(&first, start);
        let last_point = byte_point(&last, end);
        let first_position = Position {
            row: first.row,
            byte: start,
        };
        let last_position = Position {
            row: last.row,
            byte: end,
        };
        let expected = format!(
            "{}\n{TABLE}\n{SECOND}\n{THIRD}\n{}",
            &FIRST[start..],
            &LAST[..end]
        );
        for reverse in [false, true] {
            let (from, to, anchor, head) = if reverse {
                (last_point, first_point, last_position, first_position)
            } else {
                (first_point, last_point, first_position, last_position)
            };
            fixture.drag(from, to);
            fixture.capture(&format!("selection-{zoom:.1}-{reverse}.png"));
            fixture.assert_anchors(anchor, head);
            assert_eq!(fixture.copy(), expected);
            assert_glyph_coverage(&mut fixture, &first, start..FIRST.len());
            for text in [SECOND, THIRD] {
                let fragment = fixture.fragment(text);
                assert_glyph_coverage(&mut fixture, &fragment, 0..text.len());
            }
            assert_glyph_coverage(&mut fixture, &last, 0..end);
        }
        let middle = fixture.fragment(SECOND);
        let start = SECOND.find("benchmarks").unwrap();
        let end = SECOND.find("Focused").unwrap();
        for reverse in [false, true] {
            let a = byte_point(&middle, start);
            let b = byte_point(&middle, end);
            fixture.drag(if reverse { b } else { a }, if reverse { a } else { b });
            assert_eq!(fixture.copy(), SECOND[start..end].trim());
            assert_glyph_coverage(&mut fixture, &middle, start..end);
        }
    }
}

// GPUI paints aligned text at the right edge of its cell. Calculate that
// glyph position independently of TextLayout's index-to-position mapping.
fn right_aligned_point(fragment: &FragmentSnapshot, byte: usize) -> Point<Pixels> {
    let lines = fragment.layout.line_layouts();
    let line = &lines[0].unwrapped_layout;
    let x = fragment.layout.bounds().right() - line.width + line.x_for_index(byte);
    gpui::point(
        x + px(1.),
        fragment.layout.bounds().top() + fragment.layout.line_height() / 2.,
    )
}

#[test]
fn right_aligned_table_selection_tracks_the_text_and_copy() {
    let mut fixture = Fixture::new(0.9);
    let first = fixture.fragment(FIRST);
    let last = fixture.fragment(LAST);
    fixture.drag(byte_point(&first, 0), byte_point(&last, LAST.len() - 1));
    let cell = fixture.fragment("3.0 s");
    assert!(
        fixture.painted_at(right_aligned_point(&cell, 1)),
        "a selected table value must be highlighted where it is drawn"
    );
    let start = right_aligned_point(&cell, 0);
    let end = right_aligned_point(&cell, 3);
    for reverse in [false, true] {
        fixture.drag(
            if reverse { end } else { start },
            if reverse { start } else { end },
        );
        let left = Position {
            row: cell.row,
            byte: cell.offset,
        };
        let right = Position {
            row: cell.row,
            byte: cell.offset + 3,
        };
        fixture.assert_anchors(
            if reverse { right } else { left },
            if reverse { left } else { right },
        );
        assert_eq!(fixture.copy(), "3.0");
        assert!(fixture.painted_at(right_aligned_point(&cell, 1)));
        assert!(!fixture.painted_at(right_aligned_point(&cell, 4)));
    }
}

const CENTERED_CELL: &str = "Centered entries wrap over several visual lines, with enough words to check both the beginning and the end of a partial selection.";
const RIGHT_CELL: &str = "Right aligned entries also wrap over several visual lines, so their selection follows the text on every line of the cell.";
const WRAPPED_TABLE: &str = "| Name | Centered | Right |\n| --- | :---: | ---: |\n| Item | Centered entries wrap over several visual lines, with enough words to check both the beginning and the end of a partial selection. | Right aligned entries also wrap over several visual lines, so their selection follows the text on every line of the cell. |";

// Calculate a glyph's drawn position from the shaped runs and the cell's
// alignment, independently of TextLayout's selection geometry.
fn aligned_glyph_point(
    fragment: &FragmentSnapshot,
    byte: usize,
    alignment: gpui::TextAlign,
) -> Option<Point<Pixels>> {
    let lines = fragment.layout.line_layouts();
    let line = &lines[0];
    let glyph = |boundary: &gpui::WrapBoundary| {
        &line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix]
    };
    for row in 0..=line.wrap_boundaries().len() {
        let previous = row
            .checked_sub(1)
            .and_then(|index| line.wrap_boundaries().get(index))
            .map(glyph);
        let next = line.wrap_boundaries().get(row).map(glyph);
        let start = previous.map_or(0, |glyph| glyph.index);
        let end = next.map_or(line.len(), |glyph| glyph.index);
        if byte < start || byte >= end {
            continue;
        }
        let start_x = previous.map_or(px(0.), |glyph| glyph.position.x);
        let end_x = next.map_or(line.unwrapped_layout.width, |glyph| glyph.position.x);
        let available = fragment.layout.bounds().size.width - (end_x - start_x);
        let offset = match alignment {
            gpui::TextAlign::Left => px(0.),
            gpui::TextAlign::Center => available / 2.,
            gpui::TextAlign::Right => available,
        };
        let left = line.unwrapped_layout.x_for_index(byte);
        let right = line.unwrapped_layout.x_for_index(byte + 1);
        if right <= left {
            return None;
        }
        return Some(gpui::point(
            fragment.layout.bounds().left() + offset + (left + right) / 2. - start_x,
            fragment.layout.bounds().top()
                + fragment.layout.line_height() * row
                + fragment.layout.line_height() / 2.,
        ));
    }
    None
}

#[test]
fn wrapped_center_and_right_table_cells_keep_partial_ranges_aligned() {
    let mut fixture = Fixture::with_markdown(0.9, WRAPPED_TABLE);
    for (text, alignment) in [
        (CENTERED_CELL, gpui::TextAlign::Center),
        (RIGHT_CELL, gpui::TextAlign::Right),
    ] {
        let cell = fixture.fragment(text);
        assert!(cell.layout.line_layouts()[0].wrap_boundaries().len() >= 2);
        let start = 1;
        let end = text.len() - 1;
        let a = aligned_glyph_point(&cell, start, alignment).unwrap();
        let b = aligned_glyph_point(&cell, end, alignment).unwrap();
        for reverse in [false, true] {
            fixture.drag(if reverse { b } else { a }, if reverse { a } else { b });
            let left = Position {
                row: cell.row,
                byte: cell.offset + start,
            };
            let right = Position {
                row: cell.row,
                byte: cell.offset + end,
            };
            fixture.assert_anchors(
                if reverse { right } else { left },
                if reverse { left } else { right },
            );
            assert_eq!(fixture.copy(), &text[start..end]);
            for (byte, ch) in text.char_indices().filter(|(_, ch)| ch.is_ascii_graphic()) {
                if let Some(point) = aligned_glyph_point(&cell, byte, alignment) {
                    assert_eq!(
                        fixture.painted_at(point),
                        (start..end).contains(&byte),
                        "{alignment:?} table cell, byte {byte}, char {ch:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn selection_starting_at_a_wrap_does_not_highlight_the_previous_line() {
    let mut fixture = Fixture::new(0.9);
    let paragraph = fixture.fragment(SECOND);
    let lines = paragraph.layout.line_layouts();
    let line = &lines[0];
    let boundary = &line.wrap_boundaries()[0];
    let start = line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index;
    let end = start + 6;
    let a = aligned_glyph_point(&paragraph, start, gpui::TextAlign::Left).unwrap();
    let b = aligned_glyph_point(&paragraph, end, gpui::TextAlign::Left).unwrap();
    for reverse in [false, true] {
        fixture.drag(if reverse { b } else { a }, if reverse { a } else { b });
        let left = Position {
            row: paragraph.row,
            byte: start,
        };
        let right = Position {
            row: paragraph.row,
            byte: end,
        };
        fixture.assert_anchors(
            if reverse { right } else { left },
            if reverse { left } else { right },
        );
        assert_eq!(fixture.copy(), SECOND[start..end].trim());
        let previous_line_has_highlight = fixture
            .window
            .update(&mut fixture.cx, |_, window, _| {
                let scale = window.scale_factor();
                let y = (paragraph.layout.bounds().top() + paragraph.layout.line_height() / 2.)
                    .as_f32()
                    * scale;
                let left = paragraph.layout.bounds().left().as_f32() * scale;
                let right = paragraph.layout.bounds().right().as_f32() * scale;
                window.painted_quads().iter().any(|quad| {
                    quad.background == gpui::Background::from(content(0.2))
                        && quad.bounds.top().0 <= y
                        && y < quad.bounds.bottom().0
                        && quad.bounds.right().0 > left
                        && quad.bounds.left().0 < right
                })
            })
            .unwrap();
        assert!(!previous_line_has_highlight);
        assert_glyph_coverage(&mut fixture, &paragraph, start..end);
    }
}

#[test]
fn wrapped_link_keeps_its_pointer_cursor_on_every_visual_line() {
    const LINK: &str = "A linked explanation that wraps across several lines and stays interactive from the beginning of the passage through the final visible line and the closing words. More linked words keep the final line meaningful when the window changes width and the user moves the pointer.";
    const LINK_MARKDOWN: &str = "[A linked explanation that wraps across several lines and stays interactive from the beginning of the passage through the final visible line and the closing words. More linked words keep the final line meaningful when the window changes width and the user moves the pointer.](https://example.invalid)";
    let mut fixture = Fixture::with_markdown(0.9, LINK_MARKDOWN);
    let fragment = fixture.fragment(LINK);
    let lines = fragment.layout.line_layouts();
    let line = &lines[0];
    assert!(line.wrap_boundaries().len() >= 2);
    let starts = std::iter::once(0).chain(line.wrap_boundaries().iter().map(|boundary| {
        line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
    }));
    for start in starts {
        let point = aligned_glyph_point(&fragment, start + 1, gpui::TextAlign::Left).unwrap();
        let position = fixture.platform_point(point);
        fixture.event(gpui::MouseMoveEvent {
            position,
            ..Default::default()
        });
        let cursor = fixture
            .window
            .update(&mut fixture.cx, |_, window, _| window.cursor_style())
            .unwrap();
        assert_eq!(cursor, Some(gpui::CursorStyle::PointingHand));
    }
}
