//! Diagram zoom keeps one parsed vector and one raster, owned by the lightbox.
use super::*;
use crate::diagrams::{DiagramKind, Palette};
use gpui::{DevicePixels, ParsedSvg, SvgSize};

pub(super) struct DiagramImage {
    parsed: Arc<ParsedSvg>,
    pub natural: Size<Pixels>,
    width: i32,
    rendering: bool,
}

pub(super) fn load(
    kind: DiagramKind,
    source: SharedString,
    natural: Size<Pixels>,
    window: &Window,
    cx: &mut Context<ImageLightbox>,
) {
    let renderer = cx.svg_renderer();
    let palette = Palette::current();
    let width = raster_width(
        fitted_size(natural, window.viewport_size()),
        1.,
        window.scale_factor(),
        natural,
    );
    let work = cx.background_executor().spawn(async move {
        let svg = crate::diagrams::prepare(kind, &source, &palette)?;
        let parsed = renderer
            .parse_untrusted_svg(svg.as_bytes())
            .map_err(|e| e.to_string())?;
        let image = renderer
            .render_parsed(
                &parsed,
                SvgSize::Size(size(DevicePixels(width), DevicePixels(1))),
            )
            .map_err(|e| e.to_string())?;
        Ok::<_, String>((
            DiagramImage {
                parsed: Arc::new(parsed),
                natural,
                width,
                rendering: false,
            },
            image,
        ))
    });
    cx.spawn(async move |this, cx| {
        let result = work.await;
        let _ = this.update(cx, |this, cx| {
            if this.cancelled {
                return;
            }
            match result {
                Ok((diagram, image)) => {
                    this.diagram = Some(diagram);
                    this.image = Some(image);
                }
                Err(_) => this.failed = true,
            }
            cx.notify();
        });
    })
    .detach();
    cx.on_release(|this, cx| {
        if let Some(image) = this.image.take() {
            cx.drop_image(image, None);
        }
    })
    .detach();
}

// At most 64 MiB of RGBA pixels, also within the renderer's texture limit.
fn raster_width(
    display: Size<Pixels>,
    scale: f32,
    device_scale: f32,
    natural: Size<Pixels>,
) -> i32 {
    let aspect = natural.height.as_f32().max(1.) / natural.width.as_f32().max(1.);
    let limit = 8192_f32
        .min(8192. / aspect)
        .min((16_777_216. / aspect).sqrt());
    (display.width.as_f32() * scale * device_scale)
        .min(limit)
        .max(1.)
        .floor() as i32
}

impl ImageLightbox {
    pub(super) fn refresh_diagram(&mut self, device_scale: f32, cx: &mut Context<Self>) {
        let Some(diagram) = &mut self.diagram else {
            return;
        };
        let width = raster_width(
            self.image_size,
            self.view.scale,
            device_scale,
            diagram.natural,
        );
        if self.cancelled || diagram.rendering || width <= diagram.width {
            return;
        }
        diagram.rendering = true;
        let parsed = diagram.parsed.clone();
        let renderer = cx.svg_renderer();
        let work = cx.background_executor().spawn(async move {
            renderer.render_parsed(
                &parsed,
                SvgSize::Size(size(DevicePixels(width), DevicePixels(1))),
            )
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.cancelled {
                    return;
                }
                let Some(diagram) = &mut this.diagram else {
                    return;
                };
                diagram.rendering = false;
                // A failed resolution keeps the usable image and is not retried
                // on every frame. Zoom/resize can request a higher resolution.
                diagram.width = width;
                if let Ok(image) = result
                    && let Some(old) = this.image.replace(image)
                {
                    cx.drop_image(old, None);
                }
                // One job at a time: a newer zoom is picked up by the next draw.
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn diagram_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id, label, enabled| {
            crate::controls::icon_button(id, label, enabled)
                .h(px(32.))
                .min_w(px(32.))
                .rounded(px(radius::CONTROL))
                .text_color(white(if enabled { 0.9 } else { 0.3 }))
                .hover(|s| s.bg(white(0.12)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        div()
            .absolute()
            .bottom(px(20.))
            .left_0()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .p_1()
                    .rounded(px(radius::SURFACE))
                    .bg(black(0.8))
                    .border_1()
                    .border_color(white(0.15))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        button("diagram-zoom-out", "Zoom out", self.view.scale > MIN_SCALE)
                            .debug_selector(|| "diagram-zoom-out".into())
                            .child(icon("minus", 16.))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.zoom_key(1. / KEY_ZOOM_STEP, cx)),
                            ),
                    )
                    .child(
                        button("diagram-zoom-reset", "Fit diagram", true)
                            .debug_selector(|| "diagram-zoom-reset".into())
                            .w(px(64.))
                            .text_size(px(12.))
                            .child(format!("{:.0}%", self.view.scale * 100.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view(View::IDENTITY, cx);
                                cx.stop_propagation();
                            })),
                    )
                    .child(
                        button("diagram-zoom-in", "Zoom in", self.view.scale < MAX_SCALE)
                            .debug_selector(|| "diagram-zoom-in".into())
                            .child(icon("plus", 16.))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.zoom_key(KEY_ZOOM_STEP, cx)),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_rasters_are_bounded_by_pixels_and_texture_dimensions() {
        for natural in [
            size(px(1600.), px(900.)),
            size(px(100.), px(10_000.)),
            size(px(10_000.), px(100.)),
        ] {
            let display = fitted_size(natural, size(px(1000.), px(700.)));
            for scale in [1., 2., 8.] {
                let width = raster_width(display, scale, 2., natural);
                let height =
                    (width as f32 * natural.height.as_f32() / natural.width.as_f32()).ceil() as i32;
                assert!(width > 0 && width <= 8192 && height <= 8192);
                assert!(i64::from(width) * i64::from(height) <= 16_777_216);
            }
        }
    }

    #[gpui::test]
    fn svg_and_mermaid_zoom_replace_pixels_without_resetting_the_view(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(bind);
        for (kind, source) in [
            (
                DiagramKind::Svg,
                r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 400"><rect width="800" height="400" fill="#4da3f5"/></svg>"##,
            ),
            (
                DiagramKind::Mermaid,
                "flowchart LR\n A[Start] --> B[Finish]",
            ),
        ] {
            let window = cx.update(|cx| {
                cx.open_window(Default::default(), |window, cx| {
                    cx.new(|cx| {
                        ImageLightbox::new(
                            Open {
                                source: Source::Diagram {
                                    kind,
                                    source: source.into(),
                                    natural: size(px(800.), px(400.)),
                                },
                                alt: "Diagram".into(),
                                draft_session: None,
                            },
                            None,
                            window,
                            cx,
                        )
                    })
                })
                .unwrap()
            });
            let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
            visual.run_until_parked();
            visual.refresh().unwrap();
            let (parsed, original_width) = window
                .update(visual, |view, _, _| {
                    let diagram = view.diagram.as_ref().expect("loaded vector");
                    assert!(view.image.is_some());
                    (diagram.parsed.clone(), diagram.width)
                })
                .unwrap();
            let zoom = visual.debug_bounds("diagram-zoom-in").unwrap();
            visual.simulate_click(zoom.center(), Default::default());
            visual.run_until_parked();
            visual.refresh().unwrap();
            window
                .update(visual, |view, _, _| {
                    assert_eq!(view.view.scale, KEY_ZOOM_STEP);
                    let diagram = view.diagram.as_ref().unwrap();
                    assert!(
                        Arc::ptr_eq(&parsed, &diagram.parsed),
                        "zoom reuses the parsed vector"
                    );
                    assert!(diagram.width > original_width);
                    assert!(!diagram.rendering);
                })
                .unwrap();
            let reset = visual.debug_bounds("diagram-zoom-reset").unwrap();
            visual.simulate_click(reset.center(), Default::default());
            window
                .update(visual, |view, window, _| {
                    assert_eq!(view.view, View::IDENTITY);
                    window.remove_window();
                })
                .unwrap();
        }
    }
}
