//! Native document layout shared by Notes and Meetings.
use vorkspace_ui::radius;
mod selection;

use crate::markdown::{self, MarkdownRow, Part, Tone};
use gpui::{
    AnyElement, App, Context, Div, FocusHandle, Focusable, ImageSource, MouseButton, Render, Role,
    Window, div, img, prelude::*, px,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::{path::Path, sync::Arc};
use vorkspace_model::markdown_model;
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::clock::Instant;
use vorkspace_ui::theme::*;

enum Row {
    Markdown(MarkdownRow),
    Image { source: ImageSource, alt: String },
    Missing(String),
}
pub(crate) struct Document {
    rows: Vec<Row>,
    empty: &'static str,
    focus: FocusHandle,
    selection: selection::Selection,
}
impl Document {
    pub(crate) fn note(text: &str, cwd: &str, store: &Database, cx: &mut Context<Self>) -> Self {
        let mut spans = Vec::new();
        let mut open = None;
        for (event, range) in Parser::new_ext(text, Options::all()).into_offset_iter() {
            match event {
                Event::Start(Tag::Image { dest_url, .. }) => {
                    open = Some((range.start, dest_url.to_string(), String::new()))
                }
                Event::Text(value) if open.is_some() => open.as_mut().unwrap().2.push_str(&value),
                Event::End(TagEnd::Image) => {
                    if let Some((start, url, alt)) = open.take() {
                        spans.push((start..range.end, url, alt));
                    }
                }
                _ => {}
            }
        }
        let mut rows = Vec::new();
        let mut at = 0;
        for (span, url, alt) in spans {
            if span.start < at || span.end > text.len() {
                continue;
            }
            append_markdown(&mut rows, &text[at..span.start], cwd);
            let source = if url.starts_with("/note-assets/") {
                locked(store)
                    .and_then(|store| store.note_image_path(&url))
                    .map(ImageSource::from)
            } else if url.starts_with("https://")
                || url.starts_with("http://")
                || url.starts_with("data:image/")
            {
                Ok(ImageSource::from(url))
            } else {
                let path = Path::new(&url);
                Ok(ImageSource::from(if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    Path::new(cwd).join(path)
                }))
            };
            rows.push(match source {
                Ok(source) => Row::Image { source, alt },
                Err(error) => Row::Missing(error),
            });
            at = span.end;
        }
        append_markdown(&mut rows, &text[at..], cwd);
        Self::new(rows, "No description", cx)
    }

    pub(crate) fn meeting(text: &str, cx: &mut Context<Self>) -> Self {
        Self::prose(text, "No summary yet", cx)
    }

    /// Read-only prose from a provider or issue has no local file authority.
    pub(crate) fn prose(text: &str, empty: &'static str, cx: &mut Context<Self>) -> Self {
        let rows = markdown_model::parse_full_hard_breaks(text)
            .blocks
            .iter()
            .flat_map(|block| markdown::rows(&block.block))
            .map(|mut row| {
                // A model-produced document has no project/file authority.
                // Images remain inert text in the native Markdown model.
                row.infer_file_links = false;
                let restrict = |runs: &mut Vec<markdown_model::InlineRun>| {
                    for run in runs {
                        if run.style.link.as_ref().is_some_and(|url| {
                            !["https://", "http://", "mailto:"]
                                .iter()
                                .any(|scheme| url.starts_with(scheme))
                        }) {
                            run.style.link = None;
                        }
                    }
                };
                match &mut row.part {
                    Part::Text { runs, .. } => restrict(runs),
                    Part::Table { cells, .. } => cells.iter_mut().flatten().for_each(restrict),
                    _ => {}
                }
                Row::Markdown(row)
            })
            .collect();
        Self::new(rows, empty, cx)
    }

    fn new(rows: Vec<Row>, empty: &'static str, cx: &mut Context<Self>) -> Self {
        let mut joined = Vec::with_capacity(rows.len());
        for row in rows {
            // Transcript rendering chunks fences for virtualization. A flowing
            // document keeps each fence in one bordered, selectable block.
            if let Row::Markdown(MarkdownRow {
                part: Part::Code { fence, lines },
                ..
            }) = &row
                && let Some(Row::Markdown(MarkdownRow {
                    part:
                        Part::Code {
                            fence: previous,
                            lines: previous_lines,
                        },
                    ..
                })) = joined.last_mut()
                && Arc::ptr_eq(fence, previous)
                && previous_lines.end == lines.start
            {
                previous_lines.end = lines.end;
                continue;
            }
            joined.push(row);
        }
        let rows = joined;
        let text = rows
            .iter()
            .map(|row| match row {
                Row::Markdown(row) => markdown::selection_text(row).into(),
                Row::Image { alt, .. } => alt.clone().into(),
                Row::Missing(error) => error.clone().into(),
            })
            .collect();
        Self {
            rows,
            empty,
            focus: cx.focus_handle(),
            selection: selection::Selection::new(text),
        }
    }
}

/// Shared flowing prose layout. Owners retain their own scrolling and caching.
pub(crate) fn document_row(row: &MarkdownRow, body: AnyElement) -> Div {
    div()
        .w_full()
        .pt(px(row.gap))
        .min_w_0()
        .flex()
        .gap_2()
        .pl(px(
            row.indent.saturating_sub(usize::from(row.marker.is_some())) as f32 * 16.,
        ))
        .when(row.quote > 0, |d| {
            d.border_l_2().border_color(content(0.16)).pl_3()
        })
        .when_some(row.marker.clone(), |d, marker| {
            d.child(
                div()
                    .min_w(px(16.))
                    .flex_shrink_0()
                    .text_size(px(14.))
                    .line_height(px(24.))
                    .text_color(content(0.78))
                    .child(marker),
            )
        })
        .child(div().flex_1().min_w_0().child(body))
}
fn append_markdown(rows: &mut Vec<Row>, text: &str, cwd: &str) {
    for block in &markdown_model::parse_full_hard_breaks(text).blocks {
        for mut row in markdown::rows(&block.block) {
            markdown::resolve_file_links(&mut row, cwd);
            rows.push(Row::Markdown(row));
        }
    }
}
impl Focusable for Document {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Document {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.selection.begin_frame();
        div()
            .id("markdown-document")
            .track_focus(&self.focus)
            .key_context("TranscriptSelection")
            .on_action(cx.listener(|this, _: &crate::input::Copy, _, cx| {
                if let Some(text) = this.selection.copy() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.modifiers.platform && event.keystroke.key == "a" {
                    this.selection.select_all();
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if !this.selection.is_pointer_down() {
                        this.selection.clear();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event, _, cx| this.selection.move_pointer(event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, _, cx| this.selection.end_pointer(event, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event, _, cx| this.selection.end_pointer(event, cx)),
            )
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .when(self.rows.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(13.))
                        .text_color(crate::controls::page::secondary_ink())
                        .child(self.empty),
                )
            })
            .children(self.rows.iter().enumerate().map(|(ix, row)| {
                match row {
                    Row::Image { source, alt } => div()
                        .mt_3()
                        .mb_3()
                        .w_full()
                        .min_w_0()
                        .child(
                            img(source.clone())
                                .id(("document-image", ix))
                                .role(Role::Image)
                                .aria_label(alt.clone())
                                .max_w_full()
                                .max_h(px(640.))
                                .rounded(px(radius::CONTROL))
                                .object_fit(gpui::ObjectFit::Contain),
                        )
                        .into_any_element(),
                    Row::Missing(error) => div()
                        .py_2()
                        .text_size(px(12.))
                        .text_color(crate::controls::page::secondary_ink())
                        .child(error.clone())
                        .into_any_element(),
                    Row::Markdown(row) => {
                        let id: gpui::SharedString = format!("document-markdown-{ix}").into();
                        let selection = Some(self.selection.row(ix, cx.weak_entity()));
                        let body: AnyElement = match &row.part {
                            Part::Code { fence, lines } => div()
                                .id(id.clone())
                                .overflow_x_scroll()
                                .rounded(px(radius::CONTAINER))
                                .border_1()
                                .border_color(content(0.1))
                                .bg(content(0.03))
                                .py_2()
                                .child(markdown::code_lines_selected(
                                    fence,
                                    lines.clone(),
                                    None,
                                    "",
                                    false,
                                    selection,
                                ))
                                .into_any_element(),
                            _ => markdown::body_selected(
                                row,
                                id,
                                Tone::Normal,
                                None,
                                Instant::now(),
                                selection,
                            ),
                        };
                        document_row(row, body).into_any_element()
                    }
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn meeting_prose_uses_native_rows_without_file_actions_or_image_fetches(
        cx: &mut gpui::TestAppContext,
    ) {
        let text = "## Decisions\n\n**Keep** `config.json` and [website](https://example.com). [email](mailto:test@example.com) [file](file:///private/data) [script](javascript:alert). ![tracking](https://example.com/pixel.png)\n\n| Link | Detail |\n| --- | --- |\n| [local](../secrets.txt) | `secret.txt` |\n\n<script>dangerous()</script>\n\n```rust\nlet answer = 42;\n```";
        let doc = cx.new(|cx| Document::meeting(text, cx));
        doc.read_with(cx, |doc, _| {
            let mut links = Vec::new();
            let mut inline_code = 0;
            let mut code_blocks = 0;
            for row in &doc.rows {
                let Row::Markdown(row) = row else {
                    panic!("Meeting prose cannot fetch images")
                };
                assert!(!row.infer_file_links);
                let mut inspect = |runs: &[markdown_model::InlineRun]| {
                    for run in runs {
                        assert!(!run.text.contains("dangerous()"));
                        if run.style.code {
                            inline_code += 1;
                            assert!(markdown::code_link(run, row.infer_file_links).is_none());
                        }
                        if let Some(link) = &run.style.link {
                            links.push(link.clone());
                        }
                    }
                };
                match &row.part {
                    Part::Text { runs, .. } => inspect(runs),
                    Part::Table { cells, .. } => {
                        cells.iter().flatten().for_each(|runs| inspect(runs))
                    }
                    Part::Code { fence, .. } => {
                        code_blocks += 1;
                        assert!(fence.code.contains("let answer = 42;"));
                    }
                    _ => {}
                }
            }
            assert_eq!(links, ["https://example.com", "mailto:test@example.com"]);
            assert_eq!(inline_code, 2);
            assert_eq!(code_blocks, 1);
        });
    }
}
