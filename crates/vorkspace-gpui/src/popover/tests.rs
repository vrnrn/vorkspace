use super::*;

#[gpui::test]
fn a_padded_anchor_records_its_outer_edge(cx: &mut gpui::TestAppContext) {
    struct Row(AnchorSlot);
    impl gpui::Render for Row {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div()
                    .debug_selector(|| "row".into())
                    .relative()
                    .w(px(200.))
                    .h(px(40.))
                    .px_2()
                    .py_1p5()
                    .child(self.0.probe()),
            )
        }
    }
    let slot = AnchorSlot::default();
    let window = cx.add_window(|_, _| Row(slot.clone()));
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    // A popover aligned to it starts where the row does, not its padding.
    assert_eq!(slot.get(), visual.debug_bounds("row"));
}

#[test]
fn danger_rows_keep_readable_ink_when_highlighted_or_hovered() {
    for appearance in ["dark", "light"] {
        let preferences = vorkspace_ui::preferences::Preferences {
            theme: appearance.into(),
            ..Default::default()
        };
        let _restore = test_appearance(&preferences, false, false);
        for highlighted in [false, true] {
            let ink = danger_ink(highlighted);
            for fill in [
                content(0.),
                danger_fill(false),
                danger_fill(true),
                content(0.08),
            ] {
                let background = floating_fill().blend(fill);
                let contrast = test_contrast(ink, background);
                assert!(
                    contrast >= 4.5,
                    "{appearance}, highlighted={highlighted}, fill={fill:?}: {contrast}"
                );
            }
            if appearance == "dark" {
                let original: Hsla = rgb(RED_300).into();
                assert_eq!(ink, original.opacity(if highlighted { 1. } else { 0.9 }));
                let original_fill: Hsla = rgb(RED_500).into();
                assert_eq!(danger_fill(true), original_fill.opacity(0.2));
                assert_eq!(danger_fill(false), original_fill.opacity(0.15));
            } else {
                assert_eq!(ink, status(StatusColor::Danger, 1.));
            }
        }
    }
}

#[test]
fn entrance_uses_react_timing_and_the_resolved_side() {
    assert_eq!(entrance(0.), 0.);
    assert_eq!(entrance(MENU_IN), 1.);
    // CSS cubic-bezier(.16, 1, .3, 1) is already about 97% done halfway.
    assert!((entrance(MENU_IN / 2.) - 0.972).abs() < 0.002);
    for (side, from) in [
        (Side::Bottom, point(px(0.), px(-8.))),
        (Side::Top, point(px(0.), px(8.))),
        (Side::Right, point(px(-8.), px(0.))),
        (Side::Left, point(px(8.), px(0.))),
    ] {
        assert_eq!(entrance_offset(side, 0.), from);
        assert_eq!(entrance_offset(side, 1.), Point::default());
    }
}

struct MotionHost {
    side: Side,
    anchor: Point<Pixels>,
    progress: f32,
    card: AnchorSlot,
}
impl Render for MotionHost {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            popover("motion-test", Anchor::Point(self.anchor))
                .side(self.side)
                .width(180.)
                .height(100.)
                .padding(8.)
                .track(self.card.clone())
                .child(div().size(px(20.)).bg(rgb(0x19734b)))
                .render_at(self.progress, window.viewport_size()),
        )
    }
}

struct FitHost {
    content: f32,
    card: AnchorSlot,
}
impl Render for FitHost {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            popover("fit-test", Anchor::Point(point(px(100.), px(100.))))
                .max_width(300.)
                .padding(8.)
                .track(self.card.clone())
                .child(div().w(px(self.content)).h(px(20.)))
                .render_at(1., window.viewport_size()),
        )
    }
}

#[gpui::test]
fn a_max_width_card_hugs_short_content_and_caps_long_content(cx: &mut gpui::TestAppContext) {
    let card = AnchorSlot::default();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| FitHost {
                content: 50.,
                card: card.clone(),
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    // The content, its 8px padding and the 1px frame inset each side.
    assert_eq!(card.get().unwrap().size.width, px(50. + 16. + 2.));
    window
        .update(cx, |host, _, cx| {
            host.content = 500.;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(card.get().unwrap().size.width, px(300.));
}

#[gpui::test]
fn painted_card_and_content_move_and_fade_together(cx: &mut gpui::TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| MotionHost {
                side: Side::Bottom,
                anchor: point(px(300.), px(300.)),
                progress: 1.,
                card: AnchorSlot::default(),
            })
        })
        .unwrap()
    });
    for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
        for flip in [false, true] {
            for progress in [0.25, 0.75, 1.] {
                window
                    .update(cx, |host, window, cx| {
                        host.side = side;
                        host.progress = progress;
                        let viewport = window.viewport_size();
                        host.anchor = if flip {
                            match side {
                                Side::Top => point(px(300.), px(8.)),
                                Side::Bottom => point(px(300.), viewport.height - px(8.)),
                                Side::Left => point(px(8.), px(300.)),
                                Side::Right => point(viewport.width - px(8.), px(300.)),
                            }
                        } else {
                            point(px(300.), px(300.))
                        };
                        cx.notify();
                    })
                    .unwrap();
                cx.refresh().unwrap();
                window
                    .update(cx, |host, window, _| {
                        let quads = window.painted_quads();
                        let border = quads
                            .iter()
                            .find(|q| q.border_color == content(0.1).opacity(progress))
                            .expect("the card border fades with the content");
                        let marker_color = gpui::Background::from(rgb(0x19734b)).opacity(progress);
                        let marker = quads
                            .iter()
                            .find(|q| q.background == marker_color)
                            .expect("the content uses the card's opacity");
                        let origin = |q: &gpui::Quad| {
                            point(
                                px(q.bounds.left().0 / window.scale_factor()),
                                px(q.bounds.top().0 / window.scale_factor()),
                            )
                        };
                        assert_eq!(
                            origin(marker) - origin(border),
                            point(px(9.), px(9.)),
                            "content must keep its inset throughout the entrance"
                        );
                        assert_eq!(origin(border), host.card.get().unwrap().origin);
                        let viewport = window.viewport_size();
                        let p = place(
                            Rect::from_bounds(Bounds::new(host.anchor, size(px(0.), px(0.)))),
                            (180., 100.),
                            (f32::from(viewport.width), f32::from(viewport.height)),
                            &Options {
                                side,
                                ..Options::default()
                            },
                        );
                        assert_eq!(p.side, if flip { side.opposite() } else { side });
                        let top = p.top.unwrap_or_else(|| {
                            f32::from(viewport.height) - p.bottom.unwrap() - 100.
                        });
                        assert_eq!(
                            origin(border),
                            point(px(p.left).round(), px(top).round())
                                + entrance_offset(p.side, progress),
                            "the visible frame and its hit region must move together"
                        );
                    })
                    .unwrap();
            }
        }
    }
}

struct MenuHost {
    menu: Entity<Menu>,
    anchor: Point<Pixels>,
}
impl Render for MenuHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(menu_popover(&self.menu, Anchor::Point(self.anchor), cx))
    }
}

#[gpui::test]
fn selected_danger_row_repaints_on_theme_change_without_losing_focus(
    cx: &mut gpui::TestAppContext,
) {
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            cx.new(|cx| MenuHost {
                menu: cx.new(|cx| {
                    Menu::new(
                        "Actions",
                        vec![
                            MenuAction::new("delete", "Delete conversation")
                                .danger()
                                .into(),
                        ],
                        window,
                        cx,
                    )
                }),
                anchor: point(px(100.), px(100.)),
            })
        })
        .unwrap()
    });
    let mut previous_bounds = None;
    for appearance in ["dark", "light", "dark"] {
        let preferences = vorkspace_ui::preferences::Preferences {
            theme: appearance.into(),
            ..Default::default()
        };
        let _restore = test_appearance(&preferences, false, false);
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        cx.refresh().unwrap();
        vorkspace_ui::clock::settle_animations(window, cx);
        window
            .update(cx, |host, window, cx| {
                let menu = host.menu.read(cx);
                assert_eq!(menu.active, Some(0));
                assert!(menu.focus.is_focused(window));
                let bounds = menu.rows[0].get().expect("danger row stays visible");
                if let Some(previous) = previous_bounds {
                    assert_eq!(bounds, previous, "theme changes preserve menu geometry");
                }
                previous_bounds = Some(bounds);
                let fill = gpui::Background::from(danger_fill(true));
                assert!(
                    window
                        .painted_quads()
                        .iter()
                        .any(|quad| quad.background == fill),
                    "selected danger wash must repaint for {appearance}"
                );
            })
            .unwrap();
    }
}

#[gpui::test]
fn menus_and_picker_flyouts_keep_the_same_card_edge_gap(cx: &mut gpui::TestAppContext) {
    // Compare the resting gap; entrance motion deliberately starts child
    // cards a little closer to the parent before settling into this gap.
    vorkspace_ui::clock::use_test_clock(cx);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            cx.new(|cx| {
                let nested = cx.new(|cx| {
                    Menu::new(
                        "Models",
                        vec![
                            MenuAction::new("model", "A model")
                                .submenu(vec![MenuAction::new("effort", "High")])
                                .into(),
                        ],
                        window,
                        cx,
                    )
                });
                let surface = nested.read(cx).surface_region();
                let menu = cx.new(|cx| {
                    Menu::new(
                        "Settings",
                        vec![
                            MenuAction::new("models", "Models")
                                .flyout(Flyout {
                                    view: nested.into(),
                                    width: MENU_WIDTH,
                                    padding: 0.,
                                    height: None,
                                    max_height: None,
                                    focus: None,
                                    surface: Some(surface),
                                })
                                .into(),
                            MenuAction::new("reasoning", "Reasoning")
                                .submenu(vec![MenuAction::new("high", "High")])
                                .into(),
                        ],
                        window,
                        cx,
                    )
                });
                MenuHost {
                    menu,
                    anchor: point(px(100.), px(100.)),
                }
            })
        })
        .unwrap()
    });
    for style in [
        MenuStyle::Explorer,
        MenuStyle::Group,
        MenuStyle::Composer,
        MenuStyle::Settings,
        MenuStyle::Switcher,
        MenuStyle::Access,
    ] {
        for left in [false, true] {
            window
                .update(cx, |host, window, cx| {
                    host.anchor.x = if left {
                        window.viewport_size().width - px(MENU_WIDTH + 40.)
                    } else {
                        px(100.)
                    };
                    host.menu.update(cx, |menu, cx| {
                        menu.style = style;
                        menu.close_submenu(window, cx);
                    });
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
            vorkspace_ui::clock::settle_animations(window, cx);
            for ix in [0, 1] {
                window
                    .update(cx, |host, window, cx| {
                        host.menu.update(cx, |menu, cx| {
                            menu.open_submenu(ix, None, window, cx);
                        });
                    })
                    .unwrap();
                cx.refresh().unwrap();
                vorkspace_ui::clock::settle_animations(window, cx);
                window
                    .update(cx, |host, _, cx| {
                        let menu = host.menu.read(cx);
                        let parent = menu.surface.get().unwrap();
                        let child = menu.sub_region.get().unwrap();
                        let gap = if left {
                            parent.left() - child.right()
                        } else {
                            child.left() - parent.right()
                        };
                        assert_eq!(gap, px(SUBMENU_GAP), "{style:?}, row {ix}, left={left}");
                        assert_eq!(child.top(), menu.rows[ix].get().unwrap().top().round());
                        if ix == 0 {
                            let surface = menu
                                .action(0)
                                .unwrap()
                                .flyout
                                .as_ref()
                                .unwrap()
                                .surface
                                .as_ref()
                                .unwrap()
                                .get()
                                .unwrap();
                            assert_eq!(surface, child, "nested menus track their host card");
                        }
                    })
                    .unwrap();
            }
        }
    }
}

fn anchor(left: f32, top: f32, width: f32, height: f32) -> Rect {
    Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}
const MENU: (f32, f32) = (200., 150.);
const VIEWPORT: (f32, f32) = (800., 600.);
fn with(f: impl FnOnce(&mut Options)) -> Options {
    let mut o = Options::default();
    f(&mut o);
    o
}

#[test]
fn opens_below_aligned_to_the_leading_edge_and_flips_above() {
    assert_eq!(
        place(
            anchor(100., 100., 80., 20.),
            MENU,
            VIEWPORT,
            &Options::default()
        ),
        Placement {
            side: Side::Bottom,
            left: 100.,
            top: Some(126.),
            bottom: None,
            width: 200.,
            max_height: 466.,
        }
    );
    assert_eq!(
        place(
            anchor(100., 560., 80., 20.),
            MENU,
            VIEWPORT,
            &Options::default()
        ),
        Placement {
            side: Side::Top,
            left: 100.,
            top: None,
            bottom: Some(46.),
            width: 200.,
            max_height: 546.,
        }
    );
    let top = place(
        anchor(100., 400., 80., 20.),
        MENU,
        VIEWPORT,
        &with(|o| o.side = Side::Top),
    );
    assert_eq!((top.top, top.bottom), (None, Some(206.)));
}

#[test]
fn clamps_aligns_caps_and_narrows() {
    let o = Options::default();
    assert_eq!(
        place(anchor(700., 100., 80., 20.), MENU, VIEWPORT, &o).left,
        592.
    );
    let center = with(|o| o.align = Align::Center);
    assert_eq!(
        place(anchor(300., 100., 80., 20.), MENU, VIEWPORT, &center).left,
        240.
    );
    let end = with(|o| o.align = Align::End);
    assert_eq!(
        place(anchor(400., 100., 80., 20.), MENU, VIEWPORT, &end).left,
        280.
    );
    let capped = with(|o| o.max_height = Some(200.));
    assert_eq!(
        place(anchor(100., 100., 80., 20.), MENU, VIEWPORT, &capped).max_height,
        200.
    );
    let narrow = place(anchor(0., 100., 80., 20.), MENU, (180., 600.), &o);
    assert_eq!((narrow.left, narrow.width), (8., 164.));
}

#[test]
fn keeps_minimum_heights_on_screen() {
    let min = with(|o| {
        o.side = Side::Top;
        o.min_height = Some(180.);
    });
    assert_eq!(
        place(anchor(100., 100., 80., 20.), MENU, (800., 200.), &min),
        Placement {
            side: Side::Top,
            left: 100.,
            top: None,
            bottom: Some(12.),
            width: 200.,
            max_height: 180.,
        }
    );
    let tall = with(|o| {
        o.side = Side::Top;
        o.min_height = Some(442.);
        o.max_height = Some(442.);
    });
    let placed = place(anchor(100., 300., 80., 20.), MENU, (800., 400.), &tall);
    assert_eq!(placed.max_height, 384.);
    assert!(400. - placed.bottom.unwrap() - placed.max_height >= 8.);
}

#[test]
fn flyouts_hang_off_the_right_flip_left_and_clamp() {
    let flyout = with(|o| {
        o.side = Side::Right;
        o.gap = SUBMENU_GAP;
    });
    assert_eq!(
        place(
            anchor(100., 100., 200., 24.),
            (240., 300.),
            VIEWPORT,
            &flyout
        ),
        Placement {
            side: Side::Right,
            left: 304.,
            top: Some(100.),
            bottom: None,
            width: 240.,
            max_height: 492.,
        }
    );
    let flipped = place(
        anchor(200., 100., 200., 24.),
        (240., 300.),
        (420., 600.),
        &flyout,
    );
    assert_eq!((flipped.side, flipped.left), (Side::Left, 8.));
    let low = place(
        anchor(100., 560., 200., 24.),
        (240., 300.),
        VIEWPORT,
        &with(|o| o.side = Side::Right),
    );
    assert_eq!((low.top, low.max_height), (Some(292.), 300.));
}
