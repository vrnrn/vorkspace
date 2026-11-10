use super::*;

#[test]
fn dither_matches_the_react_workers_palette_and_two_pixel_bayer_cells() {
    let source = RgbaImage::from_pixel(9, 9, image::Rgba([130, 180, 210, 127]));
    let result = background_texture(&source, Effect::Dither);
    let bayer = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    for (x, y, pixel) in result.enumerate_pixels() {
        let expected = if bayer[(y / 2 % 4) as usize][(x / 2 % 4) as usize] <= 12 {
            [158, 219, 255, 127]
        } else {
            [10, 14, 17, 127]
        };
        assert_eq!(pixel.0, expected, "React worker sample at {x},{y}");
    }
    assert_eq!(background_texture(&source, Effect::None), source);
    assert_eq!(background_texture(&source, Effect::GradientBlur), source);
}
#[test]
fn dither_texture_scales_with_the_artwork_in_the_preview() {
    let source = RgbaImage::from_pixel(128, 64, image::Rgba([130, 180, 210, 255]));
    let full = render_background(&source, Effect::Dither, 128, 64, true);
    let preview = render_background(&source, Effect::Dither, 64, 32, true);
    let reduced = imageops::resize(&full, 64, 32, imageops::FilterType::Triangle);
    for x in 0..64 {
        assert_eq!(
            preview.get_pixel(x, 0).0[..3],
            reduced.get_pixel(x, 0).0[..3]
        );
    }
}
#[test]
fn effects_keep_cover_dimensions_fade_and_change_pixels() {
    let source = RgbaImage::from_fn(64, 48, |x, y| {
        image::Rgba([(x * 4) as u8, (y * 5) as u8, 90, 255])
    });
    let plain = render_background(&source, Effect::None, 41, 33, true);
    for effect in Effect::ALL {
        let result = render_background(&source, effect, 41, 33, true);
        assert_eq!(result.dimensions(), (41, 33));
        assert_eq!(result.get_pixel(0, 0)[3], 255);
        assert_eq!(result.get_pixel(20, 32)[3], 0);
        if effect != Effect::None {
            assert_ne!(result, plain, "{effect:?}");
        }
    }
    let dark = RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 100]));
    assert_eq!(
        render_background(&dark, Effect::Dither, 4, 4, true)
            .get_pixel(0, 0)
            .0,
        [0, 0, 0, 100]
    );
}
#[test]
fn background_settings_normalize_and_round_trip() {
    let mut background = Background {
        path: "a.png".into(),
        empty_opacity: f32::NAN,
        session_opacity: 4.,
        scope: Scope::Empty,
        ..Default::default()
    };
    background.normalize();
    assert_eq!(background.opacity(true), 0.24);
    assert_eq!(background.opacity(false), 0.);
    assert_eq!(background.session_opacity, 0.65);
    assert_eq!(
        serde_json::from_str::<Background>(&serde_json::to_string(&background).unwrap()).unwrap(),
        background
    );
}
#[test]
fn importing_owns_the_image_and_rejects_invalid_files() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("original.png");
    RgbaImage::from_pixel(12, 8, image::Rgba([100, 180, 90, 255]))
        .save(&source)
        .unwrap();
    let imported =
        import_image(&source, &directory.path().join("assets"), "/project", false).unwrap();
    std::fs::remove_file(source).unwrap();
    assert_eq!(
        read_image(Path::new(&imported)).unwrap().dimensions(),
        (12, 8)
    );
    let rotated = directory.path().join("rotated.png");
    RgbaImage::from_pixel(8, 12, image::Rgba([100, 180, 90, 255]))
        .save(&rotated)
        .unwrap();
    let other = import_image(
        &rotated,
        &directory.path().join("assets"),
        "/project",
        false,
    )
    .unwrap();
    assert_ne!(
        imported, other,
        "equal pixel bytes with a different shape need separate assets"
    );
    assert_eq!(read_image(Path::new(&other)).unwrap().dimensions(), (8, 12));
    let bad = directory.path().join("bad.png");
    std::fs::write(&bad, b"not an image").unwrap();
    assert!(import_image(&bad, &directory.path().join("assets"), "/project", false).is_err());
}

#[test]
fn the_rendered_bot_body_keeps_its_shape_across_print_patterns() {
    for shape in BADGE_SHAPES {
        let body =
            |pattern| render_bot_body(shape, pattern, "full", true, 7, 4, 0x223344, 0xeeddcc, 48);
        let plain = body("solid");
        assert_eq!(plain.get_pixel(24, 24).0[3], 255);
        assert_eq!(plain.get_pixel(0, 0).0[3], 0);
        for pattern in BOT_PATTERNS.into_iter().filter(|p| *p != "solid") {
            let printed = body(pattern);
            assert_ne!(printed, plain, "{shape}/{pattern}");
            assert!(printed
                .pixels()
                .zip(plain.pixels())
                .all(|(a, b)| a[3] == b[3]));
        }
    }
}
