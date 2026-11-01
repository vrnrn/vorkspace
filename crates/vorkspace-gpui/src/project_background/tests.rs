
use super::*;
use gpui::{TestAppContext, point, size};

#[test]
fn artwork_rounds_only_the_corners_on_the_card_perimeter() {
    let card = Bounds::new(point(px(20.), px(40.)), size(px(800.), px(600.)));
    let inset = Bounds::new(point(px(21.), px(41.)), size(px(798.), px(598.)));
    assert_eq!(corners(inset, false, Some(card)), Corners::all(px(9.)));
    let beside_review = Bounds::new(inset.origin, size(px(400.), inset.size.height));
    let rounded = corners(beside_review, false, Some(card));
    assert_eq!(rounded.top_left, px(9.));
    assert_eq!(rounded.bottom_left, px(9.));
    assert_eq!(rounded.top_right, px(0.));
    let below_header = Bounds::new(point(px(21.), px(70.)), size(px(798.), px(569.)));
    assert_eq!(corners(below_header, false, Some(card)).top_left, px(0.));
    assert_eq!(corners(inset, false, None), Corners::default());
    assert_eq!(corners(inset, true, None), Corners::all(px(11.)));
}

#[gpui::test]
fn new_views_share_pending_artwork_and_draw_warm_images_on_the_first_frame(
    cx: &mut TestAppContext,
) {
    struct Surface(Vec<gpui::Entity<ProjectBackground>>);
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .children(self.0.iter().cloned())
        }
    }
    let settings = Background {
        path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets/vorkspace.png").into(),
        effect: Effect::Dither,
        ..Default::default()
    };
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                Surface(
                    (0..2)
                        .map(|_| {
                            cx.new(|_| ProjectBackground {
                                settings: Some(settings.clone()),
                                empty: true,
                                ..Default::default()
                            })
                        })
                        .collect(),
                )
            })
        })
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let image = window
        .update(cx, |surface, _, cx| {
            let first = surface.0[0].read(cx).image.as_ref().unwrap();
            let second = surface.0[1].read(cx).image.as_ref().unwrap();
            assert!(
                Arc::ptr_eq(first, second),
                "concurrent views must share a single preparation job"
            );
            first.clone()
        })
        .unwrap();
    let warm = window
        .update(cx, |surface, _, cx| {
            // Drop the original views, as Close All does before its new tab
            // appears. The application's prepared artwork remains available.
            surface.0.clear();
            let warm = cx.new(|_| ProjectBackground {
                preview: true,
                settings: Some(Background {
                    empty_opacity: 0.24,
                    ..settings.clone()
                }),
                empty: true,
                ..Default::default()
            });
            surface.0.push(warm.clone());
            cx.notify();
            warm
        })
        .unwrap();
    // Draw exactly once without giving a new worker a chance to finish.
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    warm.read_with(cx, |background, _cx| {
        assert!(!background.loading);
        assert!(Arc::ptr_eq(background.image.as_ref().unwrap(), &image));
        assert!(image.size(0).width.0 > 0);
    });
    warm.update(cx, |background, cx| {
        background.configure(
            Some(Background {
                effect: Effect::None,
                ..settings.clone()
            }),
            true,
            None,
            cx,
        );
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    cx.run_until_parked();
    warm.read_with(cx, |background, _| {
        assert!(
            !Arc::ptr_eq(background.image.as_ref().unwrap(), &image),
            "different effects need different textures"
        );
    });
    warm.update(cx, |background, cx| {
        background.configure(Some(settings), true, None, cx)
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    warm.read_with(cx, |background, _| {
        assert!(!background.loading);
        assert!(
            Arc::ptr_eq(background.image.as_ref().unwrap(), &image),
            "returning to an effect reuses its prepared texture immediately"
        );
    });
}

#[gpui::test]
fn live_resize_and_opacity_reuse_the_texture_without_a_background_job(cx: &mut TestAppContext) {
    struct Surface(gpui::Entity<ProjectBackground>);
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().relative().size_full().child(self.0.clone())
        }
    }
    let image = render_image(background_texture(
        &read_image(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/vorkspace.png"
        )))
        .unwrap(),
        Effect::Dither,
    ));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                Surface(cx.new(|_| ProjectBackground {
                    preview: true,
                    settings: Some(Background {
                        path: "prepared-artwork".into(),
                        effect: Effect::Dither,
                        ..Default::default()
                    }),
                    empty: true,
                    key: Some(Key {
                        path: "prepared-artwork".into(),
                        effect: Effect::Dither,
                    }),
                    image: Some(image.clone()),
                    ..Default::default()
                }))
            })
        })
        .unwrap()
    });
    for dimensions in [size(px(640.), px(900.)), size(px(1400.), px(650.))] {
        cx.simulate_window_resize(window.into(), dimensions);
        cx.refresh().unwrap();
        window
            .update(cx, |surface, _window, cx| {
                surface.0.update(cx, |background, cx| {
                    assert!(Arc::ptr_eq(background.image.as_ref().unwrap(), &image));
                    assert!(
                        !background.loading,
                        "resize must not schedule image processing"
                    );
                    {
                        let marker = gpui_apple::metal_renderer::background_mask_marker(
                            false, true, false, true, 0.24,
                        );
                        let quads = _window.painted_quads();
                        let mask = quads
                            .iter()
                            .find(|q| q.border_color == marker)
                            .unwrap_or_else(|| {
                                panic!(
                                    "missing background mask in {} painted quads: {:?}",
                                    quads.len(),
                                    quads
                                        .iter()
                                        .map(|q| (q.bounds, q.border_color))
                                        .collect::<Vec<_>>()
                                )
                            });
                        assert_eq!(
                            mask.bounds.origin,
                            gpui::point(gpui::ScaledPixels(0.), gpui::ScaledPixels(0.))
                        );
                        assert_eq!(
                            mask.bounds.size.width.0,
                            dimensions.width.as_f32() * _window.scale_factor()
                        );
                        assert_eq!(
                            mask.bounds.size.height.0,
                            dimensions.height.as_f32() * _window.scale_factor()
                        );
                        assert_eq!(mask.corner_radii.top_left.0, 11. * _window.scale_factor());
                    }
                    let mut settings = background.settings.clone().unwrap();
                    settings.empty_opacity = 0.24;
                    settings.session_opacity = 0.33;
                    background.configure(Some(settings), true, None, cx);
                })
            })
            .unwrap();
    }
}
