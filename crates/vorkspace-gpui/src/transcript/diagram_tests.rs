use super::*;
use crate::image_lightbox::{self, ImageLightbox, Open, Source};
use gpui::{Entity, HeadlessAppContext, TestAppContext, VisualTestContext};

fn svg() -> String {
    let mut svg = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 800 400\">\n<title>Release workflow</title>\n<desc>From an idea to a shipped release.</desc>\n<defs>\n  <linearGradient id=\"sky\" x2=\"1\" y2=\"1\">\n    <stop stop-color=\"#315dc8\"/>\n    <stop offset=\"1\" stop-color=\"#8b62e8\"/>\n  </linearGradient>\n</defs>\n<rect width=\"800\" height=\"400\" rx=\"24\" fill=\"#1d2232\"/>\n",
    );
    for i in 0..60 {
        svg += &format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"2\" fill=\"#627391\" opacity=\"0.4\"/>\n",
            24 + (i * 97) % 750,
            20 + (i * 67) % 360
        );
    }
    svg += "<path d=\"M120 210 H680\" stroke=\"#727c9a\" stroke-width=\"3\"/>\n";
    for (i, label) in ["Plan", "Build", "Review", "Ship"].iter().enumerate() {
        let x = 120 + i * 185;
        svg += &format!(
            "<circle cx=\"{x}\" cy=\"210\" r=\"40\" fill=\"url(#sky)\"/>\n<text x=\"{x}\" y=\"217\" fill=\"white\" font-size=\"20\" font-family=\"Arial\" text-anchor=\"middle\">{}</text>\n<text x=\"{x}\" y=\"288\" fill=\"#e1e5f0\" font-size=\"20\" font-family=\"Arial\" text-anchor=\"middle\">{label}</text>\n",
            i + 1
        );
    }
    svg += "<text x=\"60\" y=\"76\" fill=\"#e1e5f0\" font-size=\"30\" font-family=\"Arial\">Release workflow</text>\n</svg>\n";
    svg
}

fn row(text: String) -> Row {
    Row {
        role: "assistant".into(),
        text,
        first: true,
        sent_at: None,
    }
}

#[gpui::test]
fn streaming_svg_stays_five_lines_until_the_rendered_diagram_is_ready(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| {
                let mut view = TranscriptView::new();
                view.smooth_reveal = false;
                view
            })
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let source = svg();
    let prefix = source.lines().take(6).collect::<Vec<_>>().join("\n");
    let mut height = None;
    for code in [&prefix, &source] {
        window
            .update(visual, |view, _, cx| {
                view.patch(0, vec![row(format!("```svg\n{code}"))], cx)
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        let bounds = visual.debug_bounds("svg-streaming-preview").unwrap();
        assert!(bounds.size.height < px(170.));
        if let Some(height) = height {
            assert_eq!(bounds.size.height, height);
        }
        height = Some(bounds.size.height);
    }
    window
        .update(visual, |view, _, cx| {
            view.patch(0, vec![row(format!("```svg\n{source}```"))], cx)
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("svg-streaming-preview").is_none());
    assert!(visual.debug_bounds("inline-diagram").is_some());
}

struct Surface {
    transcript: Entity<TranscriptView>,
    lightbox: Option<Entity<ImageLightbox>>,
}
impl Render for Surface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(pane_fill())
            .font_family(vorkspace_ui::fonts::UI)
            .child(div().size_full().p_5().child(self.transcript.clone()))
            .children(self.lightbox.as_ref().map(image_lightbox::overlay))
    }
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_PAGES_SCREEN_DIR"]
fn native_diagrams_capture() {
    let output = std::path::PathBuf::from(std::env::var_os("VORKSPACE_PAGES_SCREEN_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    vorkspace_ui::clock::use_test_clock(&cx);
    cx.update(image_lightbox::bind);
    for theme in ["dark", "light"] {
        cx.update(|_| {
            configure(
                &vorkspace_ui::preferences::Preferences {
                    theme: theme.into(),
                    ..Default::default()
                },
                false,
                theme == "dark",
            )
        });
        let source = svg();
        let window = cx
            .open_window(size(px(1000.), px(720.)), |_, cx| {
                cx.new(|cx| Surface {
                    transcript: cx.new(|cx| {
                        let mut view = TranscriptView::new();
                        view.smooth_reveal = false;
                        view.patch(0, vec![row(format!("```svg\n{source}"))], cx);
                        view
                    }),
                    lightbox: None,
                })
            })
            .unwrap();
        for state in ["streaming", "complete", "expanded", "zoom"] {
            window
                .update(&mut cx, |view, window, cx| {
                    match state {
                        "complete" => view.transcript.update(cx, |view, cx| {
                            view.patch(0, vec![row(format!("```svg\n{source}```"))], cx)
                        }),
                        "expanded" => {
                            view.lightbox = Some(cx.new(|cx| {
                                ImageLightbox::new(
                                    Open {
                                        source: Source::Diagram {
                                            kind: crate::diagrams::DiagramKind::Svg,
                                            source: source.clone().into(),
                                            natural: size(px(800.), px(400.)),
                                        },
                                        alt: "Release workflow".into(),
                                        draft_session: None,
                                    },
                                    None,
                                    window,
                                    cx,
                                )
                            }))
                        }
                        _ => (),
                    }
                    cx.notify();
                })
                .unwrap();
            if state == "zoom" {
                for _ in 0..3 {
                    cx.update_window(window.into(), |_, window, cx| {
                        window.dispatch_keystroke(gpui::Keystroke::parse("cmd-=").unwrap(), cx);
                    })
                    .unwrap();
                }
            }
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("svg-{theme}-{state}.png")))
                .unwrap();
        }
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}
