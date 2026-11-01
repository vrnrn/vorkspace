use super::*;
use gpui::{
    Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext,
    WindowBounds, WindowOptions,
};

#[test]
#[ignore = "Requires Metal and VORKSPACE_ANNOTATION_CAPTURE_DIR"]
fn native_annotation_capture() {
    let output =
        std::path::PathBuf::from(std::env::var_os("VORKSPACE_ANNOTATION_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sample.png");
    let mut sample = image::RgbaImage::from_pixel(800, 480, image::Rgba([235, 237, 240, 255]));
    for y in 85..395 {
        for x in 110..690 {
            let pixel = if y < 145 {
                [41, 47, 57, 255]
            } else {
                [252, 252, 252, 255]
            };
            sample.put_pixel(x, y, image::Rgba(pixel));
        }
    }
    sample.save(&path).unwrap();
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    let store = vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/missing/capture").unwrap();
    for (width, height) in [(1000., 700.), (560., 450.)] {
        let owner = cx.update(|cx| {
            cx.new(|cx| {
                crate::conversation::Conversation::new(
                    session.clone(),
                    Arc::new(std::sync::Mutex::new(
                        vorkspace_core::store::Store::open(std::path::Path::new(":memory:"))
                            .unwrap(),
                    )),
                    cx,
                )
            })
        });
        let window = cx
            .open_window(size(px(width), px(height)), |window, cx| {
                cx.new(|cx| {
                    ImageLightbox::new(
                        Open {
                            source: Source::Path(path.clone()),
                            alt: "Fixture".into(),
                            draft_session: Some(session.id),
                        },
                        (Some(owner))
                            .as_ref()
                            .map(crate::conversation::composer_features::annotation_owner),
                        window,
                        cx,
                    )
                })
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(&mut cx, |view, _, cx| {
                assert!(view.original.is_some());
                view.brush = true;
                let input = cx.new(|cx| {
                    let mut input =
                        crate::input::Input::field("Annotation text", "Type text", white(0.6), cx);
                    input.set_text("Review this".into(), cx);
                    input
                });
                let change =
                    cx.subscribe(&input, |_, _, _: &crate::input::Changed, cx| cx.notify());
                view.texts.push(EditableText {
                    position: (0.25, 0.65),
                    size: 0.035,
                    color: 0x0055ff,
                    input,
                    _change: change,
                });
                view.strokes.push(annotation::Stroke {
                    color: 0xff3b30,
                    width: 0.01,
                    points: vec![
                        (0.32, 0.45),
                        (0.45, 0.34),
                        (0.63, 0.47),
                        (0.45, 0.58),
                        (0.32, 0.45),
                    ],
                });
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let bounds = window
            .update(&mut cx, |view, _, _| view.image_bounds(view.view))
            .unwrap();
        let capture = cx.capture_screenshot(window.into()).unwrap();
        let scale_x = capture.width() as f32 / width;
        let scale_y = capture.height() as f32 / height;
        let x = ((bounds.left() + bounds.size.width * 0.45).as_f32() * scale_x) as u32;
        let y = ((bounds.top() + bounds.size.height * 0.34).as_f32() * scale_y) as u32;
        let pixel = capture.get_pixel(x, y);
        assert!(
            pixel[0] > 200 && pixel[1] < 100 && pixel[2] < 100,
            "stroke was not rendered at the image coordinate: {pixel:?}"
        );
        capture
            .save(output.join(format!("annotation-{width}.png")))
            .unwrap();
        window
            .update(&mut cx, |view, _, cx| {
                view.palette_open = true;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("annotation-picker-{width}.png")))
            .unwrap();
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}

#[test]
#[ignore = "Requires Metal"]
fn annotation_preview_matches_flattened_text_at_zoom() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("text.png");
    let original = image::RgbaImage::from_pixel(320, 120, image::Rgba([255, 255, 255, 255]));
    original.save(&path).unwrap();
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    let store = vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/missing/text-preview").unwrap();
    let owner = cx.update(|cx| {
        cx.new(|cx| {
            crate::conversation::Conversation::new(
                session.clone(),
                Arc::new(std::sync::Mutex::new(
                    vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
                )),
                cx,
            )
        })
    });
    let window = cx
        .open_window(size(px(800.), px(600.)), |window, cx| {
            cx.new(|cx| {
                ImageLightbox::new(
                    Open {
                        source: Source::Path(path.clone()),
                        alt: "Text fixture".into(),
                        draft_session: Some(session.id),
                    },
                    (Some(owner))
                        .as_ref()
                        .map(crate::conversation::composer_features::annotation_owner),
                    window,
                    cx,
                )
            })
        })
        .unwrap();
    cx.run_until_parked();
    let texts = [
        annotation::TextBox {
            position: (0.08, 0.18),
            size: 0.07,
            color: 0x000000,
            text: "Wi fi 漢字".into(),
        },
        annotation::TextBox {
            position: (0.82, 0.64),
            size: 0.07,
            color: 0x000000,
            text: "Wi".into(),
        },
    ];
    let flattened = image::load_from_memory(&annotation::png(&original, &[], &texts).unwrap())
        .unwrap()
        .into_rgba8();
    window
        .update(&mut cx, |view, _, cx| {
            assert!(view.original.is_some());
            for text in &texts {
                let input = cx.new(|cx| {
                    let mut input =
                        crate::input::Input::field("Annotation text", "Type text", white(0.6), cx);
                    input.set_text(text.text.clone(), cx);
                    input
                });
                let change =
                    cx.subscribe(&input, |_, _, _: &crate::input::Changed, cx| cx.notify());
                view.texts.push(EditableText {
                    position: text.position,
                    size: text.size,
                    color: text.color,
                    input,
                    _change: change,
                });
            }
            cx.notify();
        })
        .unwrap();
    for zoom in [1., 2.] {
        window
            .update(&mut cx, |view, _, cx| {
                view.view = View {
                    scale: zoom,
                    x: 0.,
                    y: 0.,
                };
                view.transition = None;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let bounds = window
            .update(&mut cx, |view, _, _| view.image_bounds(view.view))
            .unwrap();
        let capture = cx.capture_screenshot(window.into()).unwrap();
        let sx = capture.width() as f32 / 800.;
        let sy = capture.height() as f32 / 600.;
        // Compare the actual editor pixels with the PNG, allowing a small
        // neighborhood for device-scale rasterization and antialiasing.
        let mut preview = vec![false; (320 * 120) as usize];
        let mut saved = preview.clone();
        for y in 0..120u32 {
            for x in 0..320u32 {
                let index = (y * 320 + x) as usize;
                saved[index] = flattened.get_pixel(x, y)[0] < 220;
                let screen_x = ((bounds.left().as_f32() + (x as f32 + 0.5) * zoom) * sx) as u32;
                let screen_y = ((bounds.top().as_f32() + (y as f32 + 0.5) * zoom) * sy) as u32;
                preview[index] = capture.get_pixel(screen_x, screen_y)[0] < 220;
            }
        }
        let overlap = |from: &[bool], to: &[bool]| {
            let mut count = 0;
            let mut matched = 0;
            for y in 0..120i32 {
                for x in 0..320i32 {
                    if !from[(y * 320 + x) as usize] {
                        continue;
                    }
                    count += 1;
                    if (-1..=1).any(|dy| {
                        (-1..=1).any(|dx| {
                            let (x, y) = (x + dx, y + dy);
                            (0..320).contains(&x)
                                && (0..120).contains(&y)
                                && to[(y * 320 + x) as usize]
                        })
                    }) {
                        matched += 1;
                    }
                }
            }
            assert!(count > 30, "text must be visible at zoom {zoom}");
            assert!(
                matched * 100 >= count * 85,
                "text differs at zoom {zoom}: {matched}/{count}"
            );
        };
        overlap(&preview, &saved);
        overlap(&saved, &preview);
    }
    // A rejected edge placement must leave both the source file and the
    // user's editable input intact, even after the asynchronous save.
    let source_before = std::fs::read(&path).unwrap();
    window
        .update(&mut cx, |view, window, cx| {
            view.texts[0].position = (0.99, 0.18);
            view.save(window, cx);
            assert!(view.saving);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(&mut cx, |view, _, cx| {
            assert!(!view.saving);
            assert!(view.error.as_deref().unwrap().contains("beyond the image"));
            assert_eq!(view.texts[0].input.read(cx).content.as_ref(), "Wi fi 漢字");
            assert_eq!(view.texts[0].position, (0.99, 0.18));
            assert_eq!(view.source, Source::Path(path.clone()));
            assert_eq!(view.original.as_ref().unwrap().as_ref(), &original);
        })
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), source_before);
}

#[gpui::test]
fn drawing_tracks_image_coordinates_and_clear_across_zoom_and_resize(cx: &mut TestAppContext) {
    cx.update(crate::input::bind);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sample.png");
    image::RgbaImage::from_pixel(800, 400, image::Rgba([255, 255, 255, 255]))
        .save(&path)
        .unwrap();
    let store = vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap();
    let session = store.create("codex", "/missing/test").unwrap();
    let owner = cx.new(|cx| {
        crate::conversation::Conversation::new(session, Arc::new(std::sync::Mutex::new(store)), cx)
    });
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(800.), px(600.)),
                ))),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    ImageLightbox::new(
                        Open {
                            source: Source::Path(path),
                            alt: "Sample".into(),
                            draft_session: Some(1),
                        },
                        (Some(owner))
                            .as_ref()
                            .map(crate::conversation::composer_features::annotation_owner),
                        window,
                        cx,
                    )
                })
            },
        )
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.color, 0xff3b30);
            assert!(view.original.is_some());
        })
        .unwrap();
    let brush = visual.debug_bounds("annotation-brush").unwrap().center();
    visual.simulate_click(brush, Default::default());
    let bounds = visual.debug_bounds("lightbox-image").unwrap();
    let start = point(
        bounds.left() + bounds.size.width * 0.25,
        bounds.top() + bounds.size.height * 0.25,
    );
    let end = point(
        bounds.left() + bounds.size.width * 0.75,
        bounds.top() + bounds.size.height * 0.75,
    );
    visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Default::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.strokes.len(), 1);
            assert_eq!(view.strokes[0].color, 0xff3b30);
            assert_eq!(view.strokes[0].points, vec![(0.25, 0.25), (0.75, 0.75)]);
            view.view = View {
                scale: 2.,
                x: -40.,
                y: 20.,
            };
            view.transition = None;
        })
        .unwrap();
    visual.simulate_resize(size(px(1000.), px(700.)));
    visual.refresh().unwrap();
    let bounds = visual.debug_bounds("lightbox-image").unwrap();
    let mapped = point(
        bounds.left() + bounds.size.width * 0.6,
        bounds.top() + bounds.size.height * 0.4,
    );
    window
        .update(visual, |view, _, _| {
            let p = view.image_point(mapped, false).unwrap();
            assert!((p.0 - 0.6).abs() < 0.0001 && (p.1 - 0.4).abs() < 0.0001);
            assert_eq!(view.strokes[0].points[0], (0.25, 0.25));
        })
        .unwrap();
    window
        .update(visual, |view, _, cx| {
            view.picker
                .update(cx, |picker, cx| picker.set_color(0x0055ff, cx));
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| assert_eq!(view.color, 0x0055ff))
        .unwrap();
    let text_tool = visual.debug_bounds("annotation-text").unwrap().center();
    visual.simulate_click(text_tool, Default::default());
    let image_bounds = visual.debug_bounds("lightbox-image").unwrap();
    let position = point(
        image_bounds.left() + image_bounds.size.width * 0.5,
        image_bounds.top() + image_bounds.size.height * 0.5,
    );
    visual.simulate_click(position, Default::default());
    visual.simulate_input("Note");
    visual.refresh().unwrap();
    window
        .update(visual, |view, _, cx| {
            assert_eq!(view.texts.len(), 1);
            assert_eq!(view.texts[0].input.read(cx).content.as_ref(), "Note");
            assert_eq!(view.texts[0].color, 0x0055ff);
            assert!((view.texts[0].position.0 - 0.5).abs() < 0.001);
            assert!((view.texts[0].position.1 - 0.5).abs() < 0.001);
        })
        .unwrap();
    let before = visual.debug_bounds("annotation-text-box-0").unwrap();
    assert!((before.left() - position.x).as_f32().abs() < 1.);
    visual.simulate_resize(size(px(900.), px(650.)));
    visual.refresh().unwrap();
    let image_bounds = visual.debug_bounds("lightbox-image").unwrap();
    let after = visual.debug_bounds("annotation-text-box-0").unwrap();
    assert!(
        (after.left() - (image_bounds.left() + image_bounds.size.width * 0.5))
            .as_f32()
            .abs()
            < 1.
    );
    assert!(
        (after.top() - (image_bounds.top() + image_bounds.size.height * 0.5))
            .as_f32()
            .abs()
            < 1.
    );
    let clear = visual.debug_bounds("annotation-clear").unwrap().center();
    visual.simulate_click(clear, Default::default());
    window
        .update(visual, |view, _, _| {
            assert!(view.strokes.is_empty());
            assert!(view.texts.is_empty());
        })
        .unwrap();
    // Cancel in the same event turn in which encoding starts, before the
    // background executor can publish. The preview is deliberately kept
    // alive to prove cancellation does not rely on entity destruction.
    window
        .update(visual, |view, window, cx| {
            view.strokes.push(annotation::Stroke {
                color: view.color,
                width: 0.01,
                points: vec![(0.5, 0.5)],
            });
            view.save(window, cx);
            assert!(view.saving);
            view.cancel();
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert!(view.cancelled);
            assert!(view.saving);
            assert!(view.error.is_none());
            assert_eq!(view.strokes.len(), 1);
        })
        .unwrap();
}

#[test]
fn fit_uses_reacts_24px_padding_without_upscaling_small_images() {
    let viewport = size(px(800.), px(600.));
    assert_eq!(
        fitted_size(size(px(100.), px(50.)), viewport),
        size(px(100.), px(50.))
    );
    assert_eq!(
        fitted_size(size(px(1600.), px(1200.)), viewport),
        size(px(736.), px(552.))
    );
}

#[test]
fn zoom_keeps_the_pointer_pixel_fixed_and_clamps_pan_and_scale() {
    let viewport = size(px(800.), px(600.));
    let image = size(px(752.), px(552.));
    let pointer = point(px(500.), px(350.));
    let zoom = View::IDENTITY.zoom_at(2.5, pointer, viewport, image);
    assert_eq!(
        zoom,
        View {
            scale: 2.5,
            x: -150.,
            y: -75.
        }
    );
    assert_eq!((100. - zoom.x) / zoom.scale, 100.);
    assert_eq!((50. - zoom.y) / zoom.scale, 50.);
    assert_eq!(zoom.zoom_at(0.1, pointer, viewport, image), View::IDENTITY);
    assert_eq!(zoom.zoom_at(100., pointer, viewport, image).scale, 8.);
    assert_eq!(
        View {
            scale: 2.,
            x: 10000.,
            y: -10000.
        }
        .clamp(viewport, image),
        View {
            scale: 2.,
            x: 352.,
            y: -252.
        }
    );
    assert_eq!(
        View {
            scale: 8.,
            x: 100.,
            y: -100.
        }
        .clamp(viewport, size(px(20.), px(10.))),
        View {
            scale: 8.,
            x: 0.,
            y: 0.
        }
    );
}

#[gpui::test]
fn lightbox_handles_keyboard_pinch_wheel_drag_double_click_and_resize(cx: &mut TestAppContext) {
    cx.update(bind);
    let window = cx.update(|cx| cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)), size(px(800.), px(600.)),
            ))),
            ..Default::default()
        }, |window, cx| cx.new(|cx| {
            let mut preview = ImageLightbox::new(Open { source: Source::Path("unused.png".into()), alt: "Test image".into(), draft_session: None }, None, window, cx);
            preview.image = Some(Image::from_bytes(ImageFormat::Svg, br##"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="1200"><rect width="1600" height="1200" fill="#4da3f5"/></svg>"##.to_vec()).to_image_data(cx.svg_renderer()).unwrap());
            preview
        })).unwrap());
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    let modifier = "cmd";
    visual.simulate_keystrokes(&format!("{modifier}-="));
    window
        .update(visual, |view, _, _| assert_eq!(view.view.scale, 1.25))
        .unwrap();
    visual.simulate_keystrokes(&format!("{modifier}--"));
    window
        .update(visual, |view, _, _| assert_eq!(view.view, View::IDENTITY))
        .unwrap();
    let pointer = point(px(500.), px(350.));
    visual.simulate_mouse_move(pointer, None, Default::default());
    visual.simulate_event(gpui::PinchEvent {
        position: pointer,
        delta: 1.,
        modifiers: Default::default(),
        phase: TouchPhase::Moved,
    });
    window
        .update(visual, |view, _, _| {
            assert_eq!(
                view.view,
                View {
                    scale: 2.,
                    x: -100.,
                    y: -50.
                }
            )
        })
        .unwrap();
    visual.simulate_event(ScrollWheelEvent {
        position: pointer,
        delta: ScrollDelta::Pixels(point(px(20.), px(-30.))),
        modifiers: Default::default(),
        touch_phase: TouchPhase::Moved,
    });
    window
        .update(visual, |view, _, _| {
            assert_eq!(
                view.view,
                View {
                    scale: 2.,
                    x: -80.,
                    y: -80.
                }
            )
        })
        .unwrap();
    visual.simulate_event(ScrollWheelEvent {
        position: pointer,
        delta: ScrollDelta::Lines(point(0., 1.)),
        modifiers: Modifiers {
            control: true,
            ..Default::default()
        },
        touch_phase: TouchPhase::Moved,
    });
    window
        .update(visual, |view, _, _| {
            assert!((view.view.scale - 2. * 0.16_f32.exp()).abs() < 0.0001)
        })
        .unwrap();
    visual.simulate_keystrokes(&format!("{modifier}-0"));
    // Double-click a real image hitbox, then drag without a CSS transition.
    visual.simulate_event(gpui::MouseDownEvent {
        button: MouseButton::Left,
        position: pointer,
        click_count: 2,
        modifiers: Default::default(),
        first_mouse: false,
    });
    visual.simulate_mouse_up(pointer, MouseButton::Left, Default::default());
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.view.scale, 2.5);
            view.transition = None;
        })
        .unwrap();
    visual.simulate_mouse_down(pointer, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(
        point(px(520.), px(360.)),
        Some(MouseButton::Left),
        Default::default(),
    );
    window
        .update(visual, |view, _, _| {
            assert_eq!(
                view.view,
                View {
                    scale: 2.5,
                    x: -130.,
                    y: -65.
                }
            );
            assert!(view.transition.is_none());
        })
        .unwrap();
    visual.simulate_mouse_up(pointer, MouseButton::Left, Default::default());
    visual.simulate_resize(size(px(1000.), px(700.)));
    visual.refresh().unwrap();
    assert_eq!(
        visual.debug_bounds("image-lightbox").unwrap().size,
        size(px(1000.), px(700.))
    );
    visual.simulate_keystrokes(&format!("{modifier}-0"));
    window
        .update(visual, |view, _, _| assert_eq!(view.view, View::IDENTITY))
        .unwrap();
    visual.update(|_, cx| {
        let mut prefs = vorkspace_ui::preferences::Preferences::default();
        prefs
            .keybindings
            .insert("View: Zoom In".into(), Some("alt-z".into()));
        crate::settings::apply_keybindings(&prefs, cx);
    });
    visual.simulate_keystrokes(&format!("{modifier}-="));
    window
        .update(visual, |view, _, _| assert_eq!(view.view, View::IDENTITY))
        .unwrap();
    visual.simulate_keystrokes("alt-z");
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.view.scale, KEY_ZOOM_STEP)
        })
        .unwrap();
}
