use super::*;

#[test]
fn supported_still_formats_decode_and_animated_gif_stays_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        12,
        8,
        image::Rgb([20, 30, 40]),
    ));
    for (format, extension) in [
        (image::ImageFormat::Png, "png"),
        (image::ImageFormat::Jpeg, "jpg"),
        (image::ImageFormat::Gif, "gif"),
        (image::ImageFormat::WebP, "webp"),
    ] {
        let path = dir.path().join(format!("image.{extension}"));
        image.save_with_format(&path, format).unwrap();
        let decoded = read_image(&path).unwrap();
        assert_eq!(decoded.dimensions(), (12, 8));
    }
    let path = dir.path().join("animated.gif");
    let file = std::fs::File::create(&path).unwrap();
    let mut encoder = image::codecs::gif::GifEncoder::new(file);
    for _ in 0..2 {
        encoder
            .encode_frame(image::Frame::new(image.to_rgba8()))
            .unwrap();
    }
    drop(encoder);
    assert!(read_image(&path).unwrap_err().contains("Animated"));
    assert!(read_image(&dir.path().join("missing.png")).is_err());
}

#[test]
fn flattened_strokes_preserve_source_and_apply_selected_color_and_alpha() {
    let original = RgbaImage::from_pixel(100, 50, image::Rgba([255, 255, 255, 255]));
    let strokes = vec![
        Stroke {
            color: 0xff3b30,
            width: 0.06,
            points: vec![(0.2, 0.5), (0.8, 0.5)],
        },
        Stroke {
            color: 0x0055ff,
            width: 0.06,
            points: vec![(0.5, 0.2)],
        },
    ];
    let bytes = png(&original, &strokes, &[]).unwrap();
    let flattened = image::load_from_memory(&bytes).unwrap().into_rgba8();
    assert_eq!(original.get_pixel(50, 25).0, [255, 255, 255, 255]);
    assert_eq!(flattened.get_pixel(50, 25).0, [255, 59, 48, 255]);
    assert_eq!(flattened.get_pixel(50, 10).0, [0, 85, 255, 255]);
    assert_eq!(flattened.get_pixel(5, 5).0, [255, 255, 255, 255]);
    assert_eq!(
        image::load_from_memory(&png(&original, &[], &[]).unwrap())
            .unwrap()
            .into_rgba8(),
        original
    );
}
#[test]
fn system_layout_uses_proportional_and_fallback_runs_and_checks_edges() {
    use gpui::{FontRun, PlatformTextSystem, px};
    let fonts = gpui_apple::AppleTextSystem::new();
    let id = fonts.font_id(&gpui::font(FONT_FAMILY)).unwrap();
    let layout = |text: &str| {
        fonts.layout_line(
            text,
            px(24.),
            &[FontRun {
                len: text.len(),
                font_id: id,
            }],
        )
    };
    assert!(layout("WWWW").width > layout("iiii").width);
    let fallback = layout("Wi 漢 🐙");
    assert!(fallback.runs.iter().any(|run| run.font_id != id));
    assert!(
        fallback
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .any(|glyph| glyph.is_emoji)
    );
    let original = RgbaImage::from_pixel(320, 120, image::Rgba([255, 255, 255, 255]));
    let text = TextBox {
        position: (0.8, 0.7),
        size: 0.06,
        color: 0x000000,
        text: "Wi".into(),
    };
    let saved = image::load_from_memory(&png(&original, &[], std::slice::from_ref(&text)).unwrap())
        .unwrap()
        .into_rgba8();
    assert!(saved.pixels().any(|pixel| pixel[0] < 200));
    let colored = TextBox {
        position: (0.1, 0.2),
        size: 0.08,
        color: 0x000000,
        text: "漢 🐙".into(),
    };
    let saved = image::load_from_memory(&png(&original, &[], &[colored]).unwrap())
        .unwrap()
        .into_rgba8();
    assert!(
        saved
            .pixels()
            .any(|pixel| pixel[0] != pixel[1] || pixel[1] != pixel[2])
    );
    assert!(
        png(
            &original,
            &[],
            &[TextBox {
                position: (0.99, 0.7),
                ..text.clone()
            }]
        )
        .is_err()
    );
    assert!(
        png(
            &original,
            &[],
            &[TextBox {
                position: (0.8, 0.99),
                ..text
            }]
        )
        .is_err()
    );
    assert!(
        original
            .pixels()
            .all(|pixel| pixel.0 == [255, 255, 255, 255])
    );
}

#[test]
fn text_flattens_with_brush_and_rejects_clipped_content() {
    let original = RgbaImage::from_pixel(400, 160, image::Rgba([255, 255, 255, 255]));
    let text = TextBox {
        position: (0.1, 0.25),
        size: 0.08,
        color: 0x0055ff,
        text: "Hello".into(),
    };
    let stroke = Stroke {
        color: 0xff3b30,
        width: 0.02,
        points: vec![(0.7, 0.7)],
    };
    let flattened =
        image::load_from_memory(&png(&original, &[stroke], std::slice::from_ref(&text)).unwrap())
            .unwrap()
            .into_rgba8();
    assert_eq!(flattened.get_pixel(280, 112).0, [255, 59, 48, 255]);
    assert!(flattened.pixels().any(|p| p[2] > p[0] && p[2] > p[1]));
    assert_eq!(original.get_pixel(280, 112).0, [255, 255, 255, 255]);
    let clipped = TextBox {
        position: (0.99, 0.25),
        ..text
    };
    assert!(
        png(&original, &[], &[clipped])
            .unwrap_err()
            .contains("beyond the image")
    );
}
