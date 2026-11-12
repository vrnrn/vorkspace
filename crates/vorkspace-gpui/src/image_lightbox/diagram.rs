//! Diagram zoom keeps one parsed vector and one raster, owned by the lightbox.
use super::*;
use crate::diagrams::PaletteSnapshot;
use gpui::{DevicePixels, Hsla, ParsedSvg, SvgSize};
use vorkspace_ui::theme::pane_fill;

#[derive(Default)]
pub(super) struct DiagramImage {
    raster: Option<DiagramRaster>,
    failed: Option<(PaletteSnapshot, i32)>,
    rendering: bool,
}

struct DiagramRaster {
    palette: PaletteSnapshot,
    parsed: Arc<ParsedSvg>,
    width: i32,
    background: Hsla,
}

impl DiagramImage {
    pub(super) fn new(cx: &mut Context<ImageLightbox>) -> Self {
        cx.on_release(|this, cx| {
            if let Some(image) = this.image.take() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        Self::default()
    }

    pub(super) fn background(&self) -> Option<Hsla> {
        self.raster.as_ref().map(|raster| raster.background)
    }
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
        let (
            Some(diagram),
            Source::Diagram {
                kind,
                source,
                natural,
            },
        ) = (&mut self.diagram, &self.source)
        else {
            return;
        };
        if self.cancelled {
            return;
        }
        let palette = PaletteSnapshot::current();
        let width = raster_width(self.image_size, self.view.scale, device_scale, *natural);
        let mut raster = diagram
            .raster
            .as_mut()
            .filter(|raster| raster.palette == palette);
        // Keep the old diagram and its backdrop together while a new palette
        // renders. Other pane styling can update without reparsing the SVG.
        if let Some(raster) = &mut raster {
            raster.background = pane_fill();
        }
        diagram.failed = diagram.failed.filter(|(failed, _)| *failed == palette);
        if diagram.rendering
            || raster.as_ref().is_some_and(|raster| raster.width >= width)
            || diagram.failed.is_some_and(|(_, failed)| failed >= width)
        {
            return;
        }
        diagram.rendering = true;
        let parsed = raster.map(|raster| raster.parsed.clone());
        let kind = *kind;
        let source = source.clone();
        let renderer = cx.svg_renderer();
        let work = cx.background_executor().spawn(async move {
            let parsed = match parsed {
                Some(parsed) => parsed,
                None => {
                    let svg = crate::diagrams::prepare(kind, &source, &palette.colors())?;
                    Arc::new(
                        renderer
                            .parse_untrusted_svg(svg.as_bytes())
                            .map_err(|e| e.to_string())?,
                    )
                }
            };
            let image = renderer
                .render_parsed(
                    &parsed,
                    SvgSize::Size(size(DevicePixels(width), DevicePixels(1))),
                )
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((parsed, image))
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
                if PaletteSnapshot::current() == palette {
                    match result {
                        Ok((parsed, image)) => {
                            diagram.raster = Some(DiagramRaster {
                                palette,
                                parsed,
                                width,
                                background: pane_fill(),
                            });
                            diagram.failed = None;
                            this.failed = false;
                            if let Some(old) = this.image.replace(image) {
                                cx.drop_image(old, None);
                            }
                        }
                        Err(_) => {
                            // Retry only for a new palette or higher resolution.
                            diagram.failed = Some((palette, width));
                            this.failed = this.image.is_none();
                        }
                    }
                }
                // A newer palette or larger zoom is picked up by the next draw.
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
                .hover(|s| s.bg(vorkspace_ui::fluid::wash(white(0.12))))
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
    use crate::diagrams::DiagramKind;
    use gpui::{TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions};
    use vorkspace_ui::{preferences::Preferences, theme::configure};

    const DIAGRAMS: [(DiagramKind, &str); 2] = [
        (
            DiagramKind::Svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 400"><rect width="800" height="400" fill="currentColor"/></svg>"#,
        ),
        (
            DiagramKind::Mermaid,
            "flowchart LR\n A[Start] --> B[Finish]",
        ),
    ];

    fn open_diagram(
        kind: DiagramKind,
        source: &str,
        cx: &mut TestAppContext,
    ) -> WindowHandle<ImageLightbox> {
        cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(720.), px(420.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        ImageLightbox::new(
                            Open {
                                source: Source::Diagram {
                                    kind,
                                    source: source.to_owned().into(),
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
                },
            )
            .unwrap()
        })
    }

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
    fn svg_and_mermaid_zoom_replace_pixels_without_resetting_the_view(cx: &mut TestAppContext) {
        cx.update(bind);
        for (kind, source) in DIAGRAMS {
            let window = open_diagram(kind, source, cx);
            let visual = &mut VisualTestContext::from_window(window.into(), cx);
            visual.run_until_parked();
            visual.refresh().unwrap();
            let (parsed, original_width) = window
                .update(visual, |view, _, _| {
                    let raster = view.diagram.as_ref().unwrap().raster.as_ref().unwrap();
                    assert!(view.image.is_some());
                    (raster.parsed.clone(), raster.width)
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
                    let raster = diagram.raster.as_ref().unwrap();
                    assert!(
                        Arc::ptr_eq(&parsed, &raster.parsed),
                        "zoom reuses the parsed vector"
                    );
                    assert!(raster.width > original_width);
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

    #[gpui::test]
    fn open_diagram_follows_the_palette_and_preserves_zoom_and_pan(cx: &mut TestAppContext) {
        let preferences = Preferences {
            theme: "system".into(),
            ..Default::default()
        };
        let camera = View {
            scale: 2.,
            x: 80.,
            y: -30.,
        };
        for (kind, source) in DIAGRAMS {
            configure(&preferences, false);
            let window = open_diagram(kind, source, cx);
            let visual = &mut VisualTestContext::from_window(window.into(), cx);
            visual.run_until_parked();
            window
                .update(visual, |view, _, cx| view.set_view(camera, cx))
                .unwrap();
            visual.run_until_parked();
            let (dark, dark_background) = window
                .update(visual, |view, _, _| {
                    assert_eq!(view.view, camera);
                    (
                        view.image.clone().unwrap(),
                        view.diagram.as_ref().unwrap().background().unwrap(),
                    )
                })
                .unwrap();

            let draw = |window: &mut Window, cx: &mut App| {
                window.refresh();
                window.draw(cx).clear(cx);
            };
            configure(&preferences, true);
            visual.update(draw);
            window
                .update(visual, |view, _, _| {
                    let diagram = view.diagram.as_ref().unwrap();
                    assert!(diagram.rendering);
                    assert_eq!(diagram.background(), Some(dark_background));
                    assert_eq!(view.image.as_ref().unwrap().id, dark.id);
                })
                .unwrap();
            visual.run_until_parked();
            let light = window
                .update(visual, |view, _, _| {
                    assert_eq!(view.view, camera);
                    let diagram = view.diagram.as_ref().unwrap();
                    assert_eq!(diagram.background(), Some(pane_fill()));
                    assert_ne!(diagram.background(), Some(dark_background));
                    let light = view.image.as_ref().unwrap();
                    assert_eq!(light.size(0), dark.size(0));
                    assert!(light.as_bytes(0) != dark.as_bytes(0), "{kind:?}");
                    light.id
                })
                .unwrap();

            // Start a dark render, then restore the light palette before the
            // executor runs. The stale raster must never replace these pixels.
            configure(&preferences, false);
            visual.update(draw);
            window
                .update(visual, |view, _, _| {
                    assert!(view.diagram.as_ref().unwrap().rendering);
                })
                .unwrap();
            configure(&preferences, true);
            visual.run_until_parked();
            window
                .update(visual, |view, window, _| {
                    assert_eq!(view.view, camera);
                    assert_eq!(view.image.as_ref().unwrap().id, light);
                    assert!(!view.diagram.as_ref().unwrap().rendering);
                    window.remove_window();
                })
                .unwrap();
        }
    }
}
