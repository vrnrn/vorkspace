//! TabLabel.tsx: leave short labels clear and fade the last 40px only on overflow.
use gpui::{ContentMask, SharedString, TextAlign, TextRun, canvas, prelude::*, px};

pub fn tab_label(text: impl Into<SharedString>) -> impl IntoElement {
    let text = text.into();
    canvas(
        move |bounds, window, _| {
            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let run = TextRun {
                len: text.len(),
                font: style.font(),
                color: style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                text.clone(),
                font_size,
                std::slice::from_ref(&run),
                None,
            );
            let overflow = line.width() > bounds.size.width && bounds.size.width > px(0.);
            (line, overflow)
        },
        |bounds, (line, overflow), window, cx| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                let marker = |begin, window: &mut gpui::Window| {
                    if overflow {
                        window.paint_layer(bounds, |window| {
                            window.paint_quad(gpui::fill(bounds, gpui::black()).border_color(
                                gpui_apple::metal_renderer::horizontal_fade_marker(
                                    begin,
                                    40. * window.scale_factor(),
                                ),
                            ));
                        });
                    }
                };
                marker(true, window);
                // Keep both marker quads on separate layers from the glyphs:
                // GPUI otherwise batches a layer's quads before its text.
                window.paint_layer(bounds, |window| {
                    let _ = line.paint(
                        bounds.origin,
                        bounds.size.height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                });
                marker(false, window);
            });
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{App, Context, Render, TestAppContext, Window, div};
    use gpui_apple::metal_renderer::horizontal_fade_marker;

    struct Label {
        width: f32,
        text: &'static str,
    }
    impl Render for Label {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(self.width))
                .h(px(20.))
                .text_size(px(13.))
                .child(tab_label(self.text))
        }
    }
    #[gpui::test]
    fn short_labels_stay_clear_and_overflow_is_remeasured_after_resize(cx: &mut TestAppContext) {
        let window = cx.update(|cx: &mut App| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| Label {
                    width: 100.,
                    text: "Short",
                })
            })
            .unwrap()
        });
        let masked = |cx: &mut TestAppContext| {
            cx.refresh().unwrap();
            window
                .update(cx, |_, window, _| {
                    let quads = window.painted_quads();
                    let begin = quads.iter().find(|quad| {
                        quad.border_color
                            == horizontal_fade_marker(true, 40. * window.scale_factor())
                    });
                    if let Some(begin) = begin {
                        let end = quads
                            .iter()
                            .find(|quad| {
                                quad.border_color
                                    == horizontal_fade_marker(false, 40. * window.scale_factor())
                            })
                            .unwrap();
                        assert!(
                            begin.order < end.order,
                            "the mask must bracket the glyph layer"
                        );
                    }
                    usize::from(begin.is_some())
                })
                .unwrap()
        };
        assert_eq!(masked(cx), 0);
        window
            .update(cx, |label, _, cx| {
                label.text = "A long session title that cannot fit in this tab";
                cx.notify();
            })
            .unwrap();
        assert_eq!(masked(cx), 1);
        window
            .update(cx, |label, _, cx| {
                label.width = 700.;
                cx.notify();
            })
            .unwrap();
        assert_eq!(masked(cx), 0);
    }
}
