use super::*;

/// Count visible connected regions in the actual raster, rather than
/// inspecting the renderer's eye-placement data. Eight-way connectivity
/// includes diagonal carets/Xs; enclosed dark regions are visor slots.
fn face_regions(image: &RgbaImage, bright: bool, enclosed_only: bool) -> Vec<(u32, u32, u32, u32)> {
    let (width, height) = image.dimensions();
    let mut seen = vec![false; (width * height) as usize];
    let matches = |x, y| {
        let p = image.get_pixel(x, y).0;
        let ink = p[3] > 200 && p[0] > 100;
        ink == bright
    };
    let mut regions = Vec::new();
    for y in 0..height {
        for x in 0..width {
            if seen[(y * width + x) as usize] || !matches(x, y) {
                continue;
            }
            let mut stack = vec![(x, y)];
            seen[(y * width + x) as usize] = true;
            let (mut left, mut top, mut right, mut bottom) = (x, y, x, y);
            let mut touches_edge = false;
            while let Some((x, y)) = stack.pop() {
                left = left.min(x);
                right = right.max(x);
                top = top.min(y);
                bottom = bottom.max(y);
                touches_edge |= x == 0 || y == 0 || x + 1 == width || y + 1 == height;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                            continue;
                        }
                        let (nx, ny) = (nx as u32, ny as u32);
                        let at = (ny * width + nx) as usize;
                        if !seen[at] && matches(nx, ny) {
                            seen[at] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            if !enclosed_only || !touches_edge {
                regions.push((left, top, right, bottom));
            }
        }
    }
    regions
}

fn eye_fixture(shape: &str, face: &str, one_eye: bool, anim: BotAnimState) -> RgbaImage {
    render_badge_or_bot_with_eyes(
        true, shape, face, false, false, 12345, 4, 0x222222, 0xffffff, 128, one_eye, anim,
    )
}

#[test]
fn one_eye_faces_stay_single_and_centered_through_gaze_blink_and_delete() {
    let poses = [
        BotAnimState::default(),
        BotAnimState {
            blink: 0.08,
            ..Default::default()
        },
        BotAnimState {
            yaw: -0.65,
            pitch: 0.45,
            ..Default::default()
        },
        BotAnimState {
            yaw: 0.65,
            pitch: -0.45,
            ..Default::default()
        },
        BotAnimState {
            yaw: 0.28,
            pitch: -0.28,
            wide: 1.14,
            squash_x: 1.045,
            squash_y: 0.955,
            hop_y: -0.025,
            tex_rot: 43.,
            ..Default::default()
        },
        BotAnimState {
            crossed_eyes: true,
            ..Default::default()
        },
    ];
    for shape in BADGE_SHAPES {
        for face in BOT_FACES {
            for anim in poses {
                let image = eye_fixture(shape, face, true, anim);
                let regions = face_regions(&image, true, false);
                assert_eq!(regions.len(), 1, "{shape}/{face}, {anim:?}: {regions:?}");
                let (left, top, right, bottom) = regions[0];
                let projected = project_eye(0., -0.01, anim.yaw, anim.pitch, 128., anim.wide);
                let expected_x = (projected.cx - 64.) * anim.squash_x + 64.;
                let expected_y =
                    (projected.cy - 112.64) * anim.squash_y + 112.64 + anim.hop_y * 128.;
                assert!(((left + right + 1) as f32 * 0.5 - expected_x).abs() < 5.);
                assert!(((top + bottom + 1) as f32 * 0.5 - expected_y).abs() < 5.);
                if face == "visor" && !anim.crossed_eyes {
                    assert_eq!(
                        face_regions(&image, false, true).len(),
                        1,
                        "a one-eye visor has one slot, including during blinks"
                    );
                }
            }
        }
    }
}

#[test]
fn ordinary_faces_keep_paired_eyes_and_the_legacy_entry_point() {
    for shape in BADGE_SHAPES {
        for face in BOT_FACES {
            for crossed_eyes in [false, true] {
                let anim = BotAnimState {
                    crossed_eyes,
                    ..Default::default()
                };
                let paired = eye_fixture(shape, face, false, anim);
                let legacy = render_badge_or_bot(
                    true, shape, face, false, false, 12345, 4, 0x222222, 0xffffff, 128, anim,
                );
                assert_eq!(legacy, paired);
                let regions = if face == "visor" && !crossed_eyes {
                    face_regions(&paired, false, true)
                } else {
                    face_regions(&paired, true, false)
                };
                assert_eq!(regions.len(), 2, "{shape}/{face}, crossed={crossed_eyes}");
                let single = eye_fixture(shape, face, true, anim);
                assert_ne!(single, paired);
                assert!(
                    single
                        .pixels()
                        .zip(paired.pixels())
                        .all(|(a, b)| a[3] == b[3]),
                    "eye layout must preserve the body's silhouette"
                );
            }
        }
    }
}

#[test]
fn eye_layout_does_not_change_project_badges_or_notification_dots() {
    for is_bot in [false, true] {
        let render = |one_eye| {
            render_badge_or_bot_with_eyes(
                is_bot,
                "squircle",
                "visor",
                true,
                true,
                12345,
                4,
                0x3b82f6,
                0x0b1528,
                128,
                one_eye,
                BotAnimState::default(),
            )
        };
        let paired = render(false);
        let single = render(true);
        if !is_bot {
            assert_eq!(paired, single);
        } else {
            // Only the face changes: the complete attention dot and its
            // surroundings are identical, even on a textured body.
            for y in 0..32 {
                for x in 96..128 {
                    assert_eq!(paired.get_pixel(x, y), single.get_pixel(x, y));
                }
            }
        }
    }
}

#[test]
fn texture_cache_preserves_exact_pixels_across_look_and_rotation_changes() {
    use std::sync::Arc;
    let texture = |seed, grid, textured, bg, fg, bucket, rotation| {
        cached_concentric_texture(seed, grid, textured, bg, fg, bucket, rotation)
    };
    let resting = texture(7, 3, true, 0x223344, 0xddeeff, 96, 0.);
    assert!(Arc::ptr_eq(
        &resting,
        &texture(7, 3, true, 0x223344, 0xddeeff, 96, 0.)
    ));
    for (seed, grid, textured, bg, fg, bucket, rotation) in [
        (7, 3, true, 0x223344, 0xddeeff, 96, 10.4),
        (7, 3, true, 0x223344, 0xddeeff, 96, 9.6),
        (8, 8, true, 0x223344, 0xddeeff, 192, -1.25),
        (7, 2, false, 0x112233, 0x445566, 96, 360.),
        (7, 3, true, 0x223344, 0xddeeff, 96, 0.),
    ] {
        assert_eq!(
            *texture(seed, grid, textured, bg, fg, bucket, rotation),
            render_concentric_texture(seed, grid, textured, bg, fg, bucket, rotation)
        );
    }
}

#[test]
fn texture_cache_evicts_by_bytes_without_flushing_hot_entries() {
    use std::sync::Arc;
    let mut cache = TextureCache::default();
    let image = Arc::new(RgbaImage::new(192, 192));
    let key = |seed| ((seed, 3, true, 0, 0, 192), 0);
    for seed in 0..64 {
        cache.insert(key(seed), image.clone());
    }
    assert_eq!(cache.bytes, TEXTURE_CACHE_BYTES);
    assert!(cache.get(&key(0)).is_some());
    cache.insert(key(64), image.clone());
    assert!(cache.get(&key(1)).is_none());
    assert!(cache.get(&key(0)).is_some());
    assert_eq!(cache.entries.len(), 64);
    cache.insert(key(0), image.clone());
    assert_eq!(cache.bytes, TEXTURE_CACHE_BYTES);
    for angle in 1..180 {
        cache.insert((key(0).0, angle), image.clone());
    }
    assert_eq!(cache.bytes, TEXTURE_CACHE_BYTES);
    assert_eq!(cache.entries.len(), 64);
    assert!(cache.get(&(key(0).0, 179)).is_some());
    assert!(cache.get(&key(2)).is_some());
    cache.insert(key(65), Arc::new(RgbaImage::new(2048, 2048)));
    assert_eq!(cache.bytes, TEXTURE_CACHE_BYTES);
    assert!(cache.get(&(key(0).0, 179)).is_some());
}

#[test]
#[ignore = "explicit texture CPU benchmark"]
fn texture_cache_benchmark() {
    use std::{hint::black_box, time::Instant};
    for bucket in [96, 192] {
        for _ in 0..5 {
            let start = Instant::now();
            for _ in 0..60 {
                black_box(render_concentric_texture(
                    7, 4, true, 0x223344, 0xddeeff, bucket, 0.,
                ));
            }
            let baseline = start.elapsed();
            black_box(cached_concentric_texture(
                7, 4, true, 0x223344, 0xddeeff, bucket, 0.,
            ));
            let start = Instant::now();
            for _ in 0..60 {
                black_box(cached_concentric_texture(
                    7, 4, true, 0x223344, 0xddeeff, bucket, 0.,
                ));
            }
            eprintln!(
                "texture bucket={bucket} samples=60 baseline_us={} cached_us={}",
                baseline.as_micros(),
                start.elapsed().as_micros()
            );
        }
    }
}

#[test]
#[ignore = "explicit avatar raster CPU benchmark"]
fn avatar_raster_benchmark() {
    use std::{hint::black_box, time::Instant};
    for side in [32, 48, 128, 256] {
        for _ in 0..3 {
            let start = Instant::now();
            for i in 0..60 {
                let anim = BotAnimState {
                    yaw: (i as f32 * 0.37).sin() * 0.5,
                    pitch: (i as f32 * 0.21).cos() * 0.4,
                    ..Default::default()
                };
                black_box(render_badge_or_bot(
                    true, "squircle", "dots", false, true, 7, 4, 0x223344, 0xddeeff, side, anim,
                ));
            }
            eprintln!(
                "avatar side={side} samples=60 per_frame_us={}",
                start.elapsed().as_micros() / 60
            );
        }
    }
}

#[test]
fn delete_hover_renders_crossed_eyes_without_changing_the_badge() {
    let render = |crossed_eyes| {
        render_badge_or_bot(
            true,
            "squircle",
            "dots",
            false,
            false,
            12345,
            4,
            0x222222,
            0xffffff,
            200,
            BotAnimState {
                crossed_eyes,
                ..Default::default()
            },
        )
    };
    let normal = render(false);
    let crossed = render(true);
    // Two diagonal strokes meet at the left eye; the side of the former
    // round eye is now clear. These are artwork coordinates at 200px.
    for (x, y) in [(77, 99), (87, 109), (87, 89)] {
        assert!(crossed.get_pixel(x, y).0[0] > 200);
    }
    assert!(crossed.get_pixel(87, 99).0[0] < 100);
    assert!(normal.get_pixel(87, 99).0[0] > 200);
    assert_eq!(normal.get_pixel(40, 40), crossed.get_pixel(40, 40));
    for (normal, crossed) in normal.pixels().zip(crossed.pixels()) {
        assert_eq!(normal.0[3], crossed.0[3]);
    }
}

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
fn project_settings_normalize_and_round_trip_without_crossing_projects() {
    let mut appearance = ProjectAppearance {
        name: "  My project  ".into(),
        color: Some(0xff000000),
        mascot: Some("missing".into()),
        background: Some(Background {
            path: "a.png".into(),
            empty_opacity: f32::NAN,
            session_opacity: 4.,
            scope: Scope::Empty,
            ..Default::default()
        }),
        ..Default::default()
    };
    appearance.normalize();
    assert_eq!(appearance.label("/tmp/project"), "My project");
    assert_eq!(appearance.color, None);
    assert_eq!(appearance.mascot, None);
    let background = appearance.background.as_ref().unwrap();
    assert_eq!(background.opacity(true), 0.24);
    assert_eq!(background.opacity(false), 0.);
    assert_eq!(background.session_opacity, 0.65);
    assert_eq!(
        serde_json::from_str::<ProjectAppearance>(&serde_json::to_string(&appearance).unwrap())
            .unwrap(),
        appearance
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
