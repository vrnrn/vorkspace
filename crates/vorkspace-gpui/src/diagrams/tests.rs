
use super::*;

fn palette() -> Palette {
    Palette {
        bg: "#101010".into(),
        fg: "#eeeeee".into(),
        muted: "#999999".into(),
        line: "#cccccc".into(),
        surface: "#202020".into(),
        surface_alt: "#303030".into(),
        border: "#444444".into(),
        accent: "#ff6600".into(),
    }
}

#[test]
fn mermaid_renders_through_pele_and_resolves_its_theme() {
    let svg =
        mermaid_svg("flowchart LR\n  A[Mermaid text] --> B{Ok?}\n  B -- yes --> C([SVG])").unwrap();
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("Mermaid text"));
    assert!(svg.contains("var(--_s)"));
    let themed = themed(&svg, &palette());
    assert!(!themed.contains("var("), "{themed}");
    assert!(themed.contains("fill=\"#202020\""));
    assert!(themed.contains("stroke=\"#cccccc\""));
    // A warm context renders the next diagram too.
    assert!(
        mermaid_svg("sequenceDiagram\n  A->>B: hi")
            .unwrap()
            .contains("<svg")
    );
    let error = mermaid_svg("notADiagram at all").unwrap_err();
    assert!(error.starts_with("Mermaid:"), "{error}");
}

#[test]
fn variables_fall_back_and_colours_mix() {
    let p = palette();
    assert_eq!(
        resolve_vars("var(--pele-unknown,#123456)", &p, &HashMap::new()),
        "#123456"
    );
    assert_eq!(
        resolve_vars("var(--pele-accent,#000)", &p, &HashMap::new()),
        "#ff6600"
    );
    assert_eq!(
        mix_colors("fill=\"color-mix(in srgb,#ffffff 25%,#000000)\""),
        "fill=\"rgba(64,64,64,1.000)\""
    );
    assert_eq!(mix_colors("color-mix(oops"), "color-mix(oops");
}

/// The first pixel, BGRA.
fn first_pixel(image: &RenderImage) -> [u8; 4] {
    image.as_bytes(0).unwrap()[..4].try_into().unwrap()
}

#[test]
fn svg_blocks_reject_scripts() {
    assert!(
        prepare(
            DiagramKind::Svg,
            "<svg><script>x()</script></svg>",
            &palette()
        )
        .is_err()
    );
    assert!(prepare(DiagramKind::Svg, "<div>no</div>", &palette()).is_err());
}

#[gpui::test]
fn untrusted_svg_embeds_inline_images_but_never_reads_files(cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private.png");
    let red = image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]));
    red.save(&path).unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    red.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let inline =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png.into_inner());
    let drawing = |href: &str, extra: &str| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image {extra} xlink:href="{href}" width="4" height="4"/></svg>"#
        )
    };
    let renderer = cx.update(|cx| cx.svg_renderer());
    let untrusted = |svg: &str| {
        let svg = prepare(DiagramKind::Svg, svg, &palette()).unwrap();
        let parsed = renderer.parse_untrusted_svg(svg.as_bytes()).unwrap();
        renderer.render_parsed(&parsed, SCALE).unwrap()
    };
    let file = drawing(path.to_str().unwrap(), r#"data-href="data:ignored""#);
    // The trusted path reads it, so this drawing does reach a file.
    let trusted = renderer
        .render_single_frame(file.as_bytes(), SCALE)
        .unwrap();
    assert_ne!(first_pixel(&trusted)[3], 0);
    assert_eq!(first_pixel(&untrusted(&file))[3], 0);
    let file_uri = drawing(&format!("file://{}", path.display()), "");
    assert_eq!(first_pixel(&untrusted(&file_uri))[3], 0);
    // Nested SVG images get no further.
    let nested =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, file.as_bytes());
    let nested = drawing(&format!("data:image/svg+xml;base64,{nested}"), "");
    assert_eq!(first_pixel(&untrusted(&nested))[3], 0);
    let data = drawing(&format!("data:image/png;base64,{inline}"), "");
    assert_eq!(first_pixel(&untrusted(&data)), [0, 0, 255, 255]);
}

#[gpui::test]
fn diagrams_keep_their_logical_size_at_a_retina_raster(cx: &mut gpui::TestAppContext) {
    let renderer = cx.update(|cx| cx.svg_renderer());
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="64"><rect width="160" height="64"/></svg>"#;
    let parsed = renderer.parse_untrusted_svg(svg.as_bytes()).unwrap();
    let image = renderer.render_parsed(&parsed, SCALE).unwrap();
    assert_eq!(image.size(0).width.0, 320);
    assert_eq!(image.size(0).height.0, 128);
}

#[gpui::test]
fn raw_svg_current_color_follows_the_palette(cx: &mut gpui::TestAppContext) {
    let renderer = cx.update(|cx| cx.svg_renderer());
    let pixel = |svg: &str, palette: &Palette| {
        let svg = prepare(DiagramKind::Svg, svg, palette).unwrap();
        let parsed = renderer.parse_untrusted_svg(svg.as_bytes()).unwrap();
        first_pixel(&renderer.render_parsed(&parsed, SCALE).unwrap())
    };
    let light = Palette {
        fg: "#112233".into(),
        ..palette()
    };
    let dark = Palette {
        fg: "#ddeeff".into(),
        ..palette()
    };
    let plain = r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="currentColor"/></svg>"#;
    assert_eq!(pixel(plain, &light), [0x33, 0x22, 0x11, 255]);
    assert_eq!(pixel(plain, &dark), [0xff, 0xee, 0xdd, 255]);
    // Authored colours stay, however they were set.
    let attribute = plain.replace("<svg ", r##"<svg color="#00ff00" "##);
    assert_eq!(pixel(&attribute, &dark), [0, 255, 0, 255]);
    let styled = plain.replace("<svg ", r##"<svg style="color:#00ff00" "##);
    assert_eq!(pixel(&styled, &dark), [0, 255, 0, 255]);
    // Attributes ending in `color` are not the root's colour.
    let stop = plain.replace("<svg ", r##"<svg stop-color="#00ff00" "##);
    assert_eq!(pixel(&stop, &dark), [0xff, 0xee, 0xdd, 255]);
    let quoted = plain.replace("<svg ", r#"<svg data-note="a > color=b" "#);
    assert_eq!(pixel(&quoted, &dark), [0xff, 0xee, 0xdd, 255]);
    // Non-ASCII attributes, names or values, are scanned safely.
    for attribute in [
        r#"data-é="label""#,
        r#"data-note="é > ü""#,
        r#"é="1" 日本="2""#,
    ] {
        let unicode = plain.replace("<svg ", &format!("<svg {attribute} "));
        assert_eq!(
            pixel(&unicode, &dark),
            [0xff, 0xee, 0xdd, 255],
            "{attribute}"
        );
    }
}

#[test]
fn fence_languages_choose_diagrams() {
    assert_eq!(
        DiagramKind::for_language("mermaid"),
        Some(DiagramKind::Mermaid)
    );
    assert_eq!(
        DiagramKind::for_language("SVG title=x"),
        Some(DiagramKind::Svg)
    );
    assert_eq!(DiagramKind::for_language("rust"), None);
}

struct Diagrams(Vec<&'static str>);
impl gpui::Render for Diagrams {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().children(self.0.iter().map(|source| {
            diagram(DiagramKind::Svg, (*source).into(), None, || {
                div().into_any_element()
            })
        }))
    }
}

const FIRST: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="red"/></svg>"#;
const SECOND: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="green"/></svg>"#;

fn key(source: &'static str) -> DiagramKey {
    (DiagramKind::Svg, source.into(), Palette::current())
}

fn draw(cx: &mut gpui::TestAppContext, sources: Vec<&'static str>) -> gpui::WindowHandle<Diagrams> {
    let window = cx.update(|cx| {
        cx.set_global(Retained {
            budget: 0,
            ..Default::default()
        });
        cx.open_window(Default::default(), |_, cx| cx.new(|_| Diagrams(sources)))
            .unwrap()
    });
    cx.run_until_parked();
    window
}

fn show(
    window: gpui::WindowHandle<Diagrams>,
    sources: Vec<&'static str>,
    cx: &mut gpui::TestAppContext,
) {
    window
        .update(cx, |view, _, cx| {
            view.0 = sources;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn replaced_diagrams_release_their_rasters(cx: &mut gpui::TestAppContext) {
    let window = draw(cx, vec![FIRST]);
    cx.update(|cx| {
        assert!(cx.has_asset::<DiagramAsset>(&key(FIRST)));
        assert_eq!(cx.global::<Retained>().entries.len(), 1);
    });
    let first = image(FIRST, cx);
    show(window, vec![SECOND], cx);
    assert!(!has_sprite(window, &first, cx));
    cx.update(|cx| {
        assert!(!cx.has_asset::<DiagramAsset>(&key(FIRST)));
        assert!(cx.has_asset::<DiagramAsset>(&key(SECOND)));
        let retained = cx.global::<Retained>();
        assert_eq!(retained.entries.len(), 1);
        assert!(retained.entries.contains_key(&key(SECOND)));
        assert_eq!(
            retained.bytes,
            retained.entries[&key(SECOND)].bytes,
            "the released raster leaves the count"
        );
    });
}

#[gpui::test]
fn diagrams_in_the_latest_frame_stay_over_budget(cx: &mut gpui::TestAppContext) {
    let window = draw(cx, vec![FIRST, SECOND]);
    for _ in 0..3 {
        window.update(cx, |_, window, _| window.refresh()).unwrap();
        cx.run_until_parked();
    }
    cx.update(|cx| {
        assert!(cx.has_asset::<DiagramAsset>(&key(FIRST)));
        assert!(cx.has_asset::<DiagramAsset>(&key(SECOND)));
        assert_eq!(cx.global::<Retained>().entries.len(), 2);
    });
}

/// A cached view inside another, as a transcript sits in its conversation.
struct Inner {
    source: &'static str,
    renders: Rc<Cell<usize>>,
}
impl gpui::Render for Inner {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .size_full()
            .child(diagram(DiagramKind::Svg, self.source.into(), None, || {
                div().into_any_element()
            }))
    }
}
struct Outer(Option<gpui::Entity<Inner>>);
impl gpui::Render for Outer {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().children(self.0.clone().map(|inner| {
            inner
                .cached(gpui::StyleRefinement::default().size_full())
                .into_any_element()
        }))
    }
}

struct Cached {
    window: gpui::WindowHandle<Outer>,
    inner: gpui::Entity<Inner>,
    renders: Rc<Cell<usize>>,
}

fn cached(cx: &mut gpui::TestAppContext) -> Cached {
    let renders = Rc::new(Cell::new(0));
    let (window, inner) = cx.update(|cx| {
        cx.set_global(Retained {
            budget: 0,
            ..Default::default()
        });
        let inner = cx.new(|_| Inner {
            source: FIRST,
            renders: renders.clone(),
        });
        let shown = inner.clone();
        let window = cx
            .open_window(Default::default(), |_, cx| cx.new(|_| Outer(Some(shown))))
            .unwrap();
        (window, inner)
    });
    cx.run_until_parked();
    Cached {
        window,
        inner,
        renders,
    }
}

fn image(source: &'static str, cx: &mut gpui::TestAppContext) -> Arc<RenderImage> {
    cx.update(|cx| cx.global::<Retained>().entries[&key(source)].image.clone())
}

fn has_sprite<V: 'static>(
    window: gpui::WindowHandle<V>,
    image: &RenderImage,
    cx: &mut gpui::TestAppContext,
) -> bool {
    window
        .update(cx, |_, window, _| window.has_image_sprite(image))
        .unwrap()
}

/// Passes the budget by drawing a diagram in a window of its own.
fn draw_elsewhere(cx: &mut gpui::TestAppContext) {
    let other = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| Diagrams(vec![SECOND]))
        })
        .unwrap()
    });
    cx.run_until_parked();
    other.update(cx, |_, window, _| window.refresh()).unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn a_cached_view_keeps_the_diagram_it_replays(cx: &mut gpui::TestAppContext) {
    let test = cached(cx);
    let first = image(FIRST, cx);
    assert!(has_sprite(test.window, &first, cx));
    // The parent redraws and replays the child without rendering it.
    let renders = test.renders.get();
    test.window.update(cx, |_, _, cx| cx.notify()).unwrap();
    cx.run_until_parked();
    assert_eq!(test.renders.get(), renders);
    cx.update(|cx| {
        let shown = cx.views_in_latest_frames().unwrap();
        assert!(shown.contains(&test.inner.entity_id()));
    });
    draw_elsewhere(cx);
    cx.update(|cx| {
        assert!(cx.has_asset::<DiagramAsset>(&key(FIRST)));
        assert!(cx.global::<Retained>().entries.contains_key(&key(FIRST)));
    });
    assert!(has_sprite(test.window, &first, cx));
}

#[gpui::test]
fn a_hidden_view_gives_up_its_raster_and_sprite(cx: &mut gpui::TestAppContext) {
    let test = cached(cx);
    let first = image(FIRST, cx);
    assert!(has_sprite(test.window, &first, cx));
    test.window
        .update(cx, |outer, _, cx| {
            outer.0 = None;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let shown = cx.views_in_latest_frames().unwrap();
        assert!(!shown.contains(&test.inner.entity_id()));
    });
    draw_elsewhere(cx);
    cx.update(|cx| {
        assert!(!cx.has_asset::<DiagramAsset>(&key(FIRST)));
        let retained = cx.global::<Retained>();
        assert!(!retained.entries.contains_key(&key(FIRST)));
        assert_eq!(retained.bytes, retained.entries[&key(SECOND)].bytes);
        assert!(!retained.views.contains_key(&test.inner.entity_id()));
    });
    assert!(!has_sprite(test.window, &first, cx));
    assert_eq!(
        Arc::strong_count(&first),
        1,
        "only this test holds the raster"
    );

    // Coming back renders afresh: no replay can reach the released sprite.
    let renders = test.renders.get();
    let inner = test.inner.clone();
    test.window
        .update(cx, |outer, _, cx| {
            outer.0 = Some(inner);
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert!(test.renders.get() > renders);
    let returned = image(FIRST, cx);
    assert_ne!(returned.id, first.id);
    assert!(has_sprite(test.window, &returned, cx));
    assert!(!has_sprite(test.window, &first, cx));
    cx.update(|cx| {
        let retained = cx.global::<Retained>();
        let total: usize = retained.entries.values().map(|entry| entry.bytes).sum();
        assert_eq!(retained.bytes, total);
    });
}

#[gpui::test]
fn the_asset_rasterizes_a_themed_diagram(cx: &mut gpui::TestAppContext) {
    let key = (
        DiagramKind::Mermaid,
        SharedString::from("flowchart TD\n  A --> B"),
        palette(),
    );
    let renderer = cx.update(|cx| cx.svg_renderer());
    let svg = prepare(key.0, &key.1, &key.2).unwrap();
    let parsed = renderer.parse_untrusted_svg(svg.as_bytes()).unwrap();
    let image = renderer.render_parsed(&parsed, SCALE).unwrap();
    assert!(image.size(0).width.0 > 0);
}
