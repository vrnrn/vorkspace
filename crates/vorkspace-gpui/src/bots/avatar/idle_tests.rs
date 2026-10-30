use super::*;
use gpui::{AppContext, Context, Entity, Render, TestAppContext, div};
use std::{cell::Cell, cell::RefCell, collections::BTreeSet, rc::Rc};

struct Avatars {
    bot: Bot,
    status: &'static str,
    renders: Rc<Cell<usize>>,
    preview: bool,
}

impl Render for Avatars {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let bot = &self.bot;
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(sidebar_cursor_tracker())
            .child(header_avatar(
                BotLookConfig::from_bot(bot),
                self.status,
                64.,
                true,
            ))
            .child(bot_avatar(bot, self.status, 24., false))
            .when(self.preview, |d| {
                d.child(settings_preview(bot, 128., Default::default(), window, cx))
            })
    }
}

fn open(
    cx: &mut TestAppContext,
) -> (
    gpui::WindowHandle<Avatars>,
    Entity<Avatars>,
    Rc<Cell<usize>>,
) {
    let renders = Rc::new(Cell::new(0));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            window.activate_window();
            cx.new(|_| Avatars {
                // Product identities persist across renders. Creating a fresh
                // random Bot each frame also randomizes the animation schedule.
                bot: Bot {
                    id: "idle-avatar-fixture".into(),
                    ..Default::default()
                },
                status: "idle",
                renders: renders.clone(),
                preview: true,
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    let view = window.root(cx).unwrap();
    cx.refresh().unwrap();
    window
        .update(cx, |_, window, _| assert!(window.is_window_active()))
        .unwrap();
    (window, view, renders)
}

/// The single pending wake of the test view, if any.
fn pending_wake() -> Option<Instant> {
    PENDING_FRAMES.with(|pending| {
        let pending = pending.borrow();
        assert!(pending.len() <= 1, "One view owns one pending wake");
        pending.values().next().map(|wake| wake.deadline)
    })
}

/// Count the view's notifications: each is one requested window frame.
fn notices(cx: &mut TestAppContext, view: &Entity<Avatars>) -> Rc<Cell<usize>> {
    let count = Rc::new(Cell::new(0));
    let observed = count.clone();
    cx.update(|cx| cx.observe(view, move |_, _| observed.set(observed.get() + 1)))
        .detach();
    count
}

/// Advance the executor to each pending wake for `span`, returning the gaps.
fn run_wakes(cx: &mut TestAppContext, span: Duration) -> Vec<Duration> {
    let end = Instant::now() + span;
    let mut gaps = Vec::new();
    while let Some(at) = pending_wake().filter(|at| *at <= end) {
        let gap = at.saturating_duration_since(Instant::now());
        cx.background_executor.advance_clock(gap);
        cx.run_until_parked();
        cx.refresh().unwrap();
        gaps.push(gap);
    }
    gaps
}

#[gpui::test]
fn sidebar_pointer_events_notify_the_painted_owner(cx: &mut TestAppContext) {
    // Suppressed clock frames isolate pointer-driven invalidation.
    crate::clock::use_test_clock(cx);
    let (window, view, _) = open(cx);
    // Isolate the rail listener: the preview has its own pointer listener.
    view.update(cx, |view, cx| {
        view.preview = false;
        cx.notify();
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    let notices = Rc::new(Cell::new(0));
    let observed = notices.clone();
    let _observer = cx.update(|cx| cx.observe(&view, move |_, _| observed.set(observed.get() + 1)));
    let cursor = window
        .update(cx, |_, window, _| sidebar_cursor(window).unwrap())
        .unwrap();
    let bounds = cursor.borrow().rail_bounds.unwrap();
    cursor.borrow_mut().mouse_pos = None;
    let inside = bounds.center();
    let outside = gpui::point(bounds.right() + px(100.), bounds.center().y);
    for (position, notify) in [
        (inside, true),
        (inside, false),
        (outside, true),
        (outside, false),
    ] {
        let before = notices.get();
        window
            .update(cx, |_, w, cx| w.simulate_mouse_move(position, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(notices.get(), before + usize::from(notify));
        assert_eq!(cursor.borrow().mouse_pos, Some(position));
        PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
    }
}

#[gpui::test]
fn sidebar_gaze_is_isolated_and_cleaned_up_per_window(cx: &mut TestAppContext) {
    crate::clock::use_test_clock(cx);
    let (first, first_view, _) = open(cx);
    let (second, second_view, _) = open(cx);
    for view in [&first_view, &second_view] {
        view.update(cx, |view, cx| {
            view.preview = false;
            cx.notify();
        });
    }
    cx.refresh().unwrap();
    cx.run_until_parked();
    let first_cursor = first
        .update(cx, |_, window, _| sidebar_cursor(window).unwrap())
        .unwrap();
    let second_cursor = second
        .update(cx, |_, window, _| sidebar_cursor(window).unwrap())
        .unwrap();
    assert!(!Rc::ptr_eq(&first_cursor, &second_cursor));
    let first_bounds = first_cursor.borrow().rail_bounds.unwrap();
    let second_bounds = second_cursor.borrow().rail_bounds.unwrap();
    let first_point = first_bounds.center();
    let second_point = gpui::point(second_bounds.right() + px(100.), second_bounds.center().y);
    first
        .update(cx, |_, window, cx| {
            window.simulate_mouse_move(first_point, cx)
        })
        .unwrap();
    second
        .update(cx, |_, window, cx| {
            window.simulate_mouse_move(second_point, cx)
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(first_cursor.borrow().mouse_pos, Some(first_point));
    assert_eq!(second_cursor.borrow().mouse_pos, Some(second_point));
    let first_handle: gpui::AnyWindowHandle = first.into();
    cx.update_window(first_handle, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    SIDEBAR_CURSORS.with(|cursors| assert!(!cursors.borrow().contains_key(&first_handle)));
    assert!(
        second
            .update(cx, |_, window, _| sidebar_cursor(window).is_some())
            .unwrap()
    );
}

#[gpui::test]
fn idle_surfaces_share_sparse_beat_wakes_instead_of_a_frame_loop(cx: &mut TestAppContext) {
    let _clock = crate::clock::use_test_clock_with_endless_frames(cx);
    let (_window, view, _) = open(cx);
    let frames = notices(cx, &view);
    let span = Duration::from_secs(120);
    let gaps = run_wakes(cx, span);
    let frames = frames.get();
    // Every wake notifies the one owning view: header, sidebar and preview
    // share it. Motion never wakes faster than the frame grid.
    assert_eq!(frames, gaps.len());
    assert!(
        gaps.iter()
            .all(|gap| *gap <= AVATAR_FRAME || *gap > AVATAR_FRAME * 3)
    );
    // A 30 fps loop would paint 3,636 frames; eventful beats cost about ten.
    let beats = (span.as_millis() as u64 / IDLE_BEAT_MS) as usize;
    assert!(frames <= beats * 12, "{frames} frames over {beats} beats");
    let sleeps = gaps.iter().filter(|gap| **gap > AVATAR_FRAME * 3).count();
    assert!(sleeps >= beats / 3, "Idle life must sleep between beats");
    assert!(frames >= beats, "Idle Bots still blink and glance");
}

#[gpui::test]
fn working_and_attention_ticks_return_to_sparse_idle_life(cx: &mut TestAppContext) {
    let _clock = crate::clock::use_test_clock_with_endless_frames(cx);
    let (_window, view, _) = open(cx);
    let renders = notices(cx, &view);
    for status in ["working", "needs-you"] {
        view.update(cx, |view, cx| {
            view.status = status;
            cx.notify();
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        let before = renders.get();
        let gaps = run_wakes(cx, Duration::from_secs(2));
        assert!(gaps.iter().all(|gap| *gap <= AVATAR_FRAME));
        assert!(
            renders.get() - before >= 55,
            "{status} animates at about 30 fps"
        );
        view.update(cx, |view, cx| {
            view.status = "idle";
            cx.notify();
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        let before = renders.get();
        run_wakes(cx, Duration::from_secs(20));
        let idle = renders.get() - before;
        assert!(
            idle <= 5 * 12 + 1,
            "{idle} idle frames in 20 s after {status}"
        );
    }
}

#[test]
fn idle_life_paints_every_pose_change_and_nothing_while_asleep() {
    let seeds: Vec<u32> = (0..12)
        .map(|i| default_seed_for(&format!("bot-{i}")))
        .collect();
    let look = BotLookConfig::from_legacy("robot", 0x3b82f6);
    let key = |life: &IdleLife| {
        let anim = BotAnimState {
            yaw: life.look.0,
            pitch: life.look.1,
            blink: life.blink,
            ..Default::default()
        };
        frame_cache_key(true, &look, false, 48, anim)
    };
    let span = Duration::from_secs(300);
    let mut union = BTreeSet::new();
    for &seed in &seeds {
        let (mut glanced, mut blinked) = (false, false);
        let mut at = Duration::ZERO;
        while at < span {
            let life = idle_life(seed, at);
            assert!(life.next > at);
            glanced |= life.look != IDLE_REST;
            blinked |= life.blink < 0.5;
            if life.next - at > AVATAR_FRAME {
                // Asleep: the painted pose must hold until the next wake.
                let painted = key(&life);
                let mut t = at;
                while t < life.next {
                    assert!(key(&idle_life(seed, t)) == painted, "seed {seed} at {t:?}");
                    t += Duration::from_millis(11);
                }
            }
            union.insert(life.next);
            at = life.next;
        }
        assert!(glanced && blinked, "seed {seed} shows idle life");
    }
    // Avatars share the beat and frame grids, so a whole sidebar costs about
    // what one avatar does: at most one motion burst per beat.
    let beats = (span.as_millis() as u64 / IDLE_BEAT_MS) as usize;
    // The beat-start wake, the grid ticks spanning the glance, and the settled frame.
    let burst = IDLE_GLANCE_MS.div_ceil(AVATAR_FRAME.as_millis() as u64) as usize + 2;
    assert!(union.len() <= beats * burst, "{} shared wakes", union.len());
}

#[test]
fn image_cache_evicts_least_recent_under_its_byte_budget() {
    let image = || to_render_image(image::RgbaImage::new(8, 8));
    // Four 256-byte images fit; overflow trims to three quarters.
    let mut cache = ImageCache::new(1024);
    for key in 0..4 {
        assert!(cache.insert(key, image()).is_empty());
    }
    assert!(cache.get(&0).is_some());
    let evicted = cache.insert(4, image());
    assert_eq!(evicted.len(), 2);
    assert!(cache.get(&1).is_none() && cache.get(&2).is_none());
    for key in [0, 3, 4] {
        assert!(cache.get(&key).is_some());
    }
    assert_eq!(cache.bytes, 768);
    // Replacing a key retires its previous image.
    let previous = cache.get(&3).unwrap();
    let replaced = cache.insert(3, image());
    assert!(replaced.len() == 1 && Arc::ptr_eq(&replaced[0], &previous));
    assert_eq!(cache.bytes, 768);
    // An oversized one-off is retired without evicting the working set.
    let huge = to_render_image(image::RgbaImage::new(32, 32));
    let retired = cache.insert(9, huge.clone());
    assert!(retired.len() == 1 && Arc::ptr_eq(&retired[0], &huge));
    assert!(cache.get(&9).is_none());
    assert_eq!(cache.entries.len(), 3);
}

struct AtlasProbe {
    cache: Rc<Mutex<ImageCache<u32>>>,
    painted: Rc<RefCell<Vec<Arc<RenderImage>>>>,
    keys: std::ops::Range<u32>,
}

impl Render for AtlasProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (cache, painted, keys) = (self.cache.clone(), self.painted.clone(), self.keys.clone());
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                for key in keys.clone() {
                    let image = cached_image(&cache, key, window, cx, || {
                        to_render_image(image::RgbaImage::new(8, 8))
                    });
                    let _ = window.paint_image(
                        bounds,
                        bounds,
                        Corners::default(),
                        image.clone(),
                        0,
                        false,
                    );
                    painted.borrow_mut().push(image);
                }
            },
        )
        .size(px(20.))
    }
}

#[gpui::test]
fn evicted_images_stay_in_the_atlas_while_the_scene_uses_them(cx: &mut TestAppContext) {
    let cache = Rc::new(Mutex::new(ImageCache::new(1024)));
    let painted = Rc::new(RefCell::new(Vec::new()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| AtlasProbe {
                cache: cache.clone(),
                painted: painted.clone(),
                keys: 0..6,
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    let images = painted.borrow().clone();
    assert_eq!(images.len(), 6);
    let resident = |cx: &mut TestAppContext, image: &Arc<RenderImage>| {
        window
            .update(cx, |_, window, _| window.has_image_atlas_entry(image))
            .unwrap()
    };
    // The frame that painted them may still reference them.
    for image in &images {
        assert!(resident(cx, image));
    }
    window
        .update(cx, |_, window, cx| window.simulate_next_frame(cx))
        .unwrap();
    for image in &images {
        assert!(resident(cx, image));
    }
    let retired_ids = [images[0].id, images[1].id];
    painted.borrow_mut().clear();
    drop(images);
    window
        .update(cx, |view, _, cx| {
            view.keys = 2..6;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            window.simulate_next_frame(cx);
        })
        .unwrap();
    cx.run_until_parked();
    RETIRED_IMAGES.with(|retired| {
        for id in retired_ids {
            assert!(
                !retired.borrow().contains_key(&id),
                "unused image must be released"
            );
        }
    });
}

#[test]
#[ignore = "requires Metal"]
fn evicted_image_remains_correct_in_retained_scene() {
    struct Tile {
        cache: Rc<Mutex<ImageCache<u32>>>,
        key: u32,
        color: [u8; 4],
        renders: Rc<Cell<usize>>,
    }
    impl Render for Tile {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            let (cache, key, color) = (self.cache.clone(), self.key, self.color);
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    for id in key..if (3..11).contains(&key) {
                        key + 4
                    } else {
                        key + 1
                    } {
                        let image = cached_image(&cache, id, window, cx, || {
                            to_render_image(image::RgbaImage::from_pixel(
                                512,
                                512,
                                image::Rgba(color),
                            ))
                        });
                        window
                            .paint_image(bounds, bounds, Corners::default(), image, 0, false)
                            .unwrap();
                    }
                },
            )
            .size(px(20.))
        }
    }
    struct Tiles {
        left: Entity<Tile>,
        right: Entity<Tile>,
    }
    impl Render for Tiles {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_row()
                .child(
                    self.left
                        .clone()
                        .cached(gpui::StyleRefinement::default().size(px(20.))),
                )
                .child(
                    self.right
                        .clone()
                        .cached(gpui::StyleRefinement::default().size(px(20.))),
                )
        }
    }
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    let cache = Rc::new(Mutex::new(ImageCache::new(512 * 512 * 4)));
    let left_renders = Rc::new(Cell::new(0));
    let left = cx.new(|_| Tile {
        cache: cache.clone(),
        key: 1,
        color: [255, 0, 0, 255],
        renders: left_renders.clone(),
    });
    let right = cx.new(|_| Tile {
        cache: cache.clone(),
        key: 2,
        color: [0, 255, 0, 255],
        renders: Rc::new(Cell::new(0)),
    });
    let window = cx
        .open_window(gpui::size(px(40.), px(20.)), |_, cx| {
            cx.new(|_| Tiles {
                left: left.clone(),
                right: right.clone(),
            })
        })
        .unwrap();
    cx.run_until_parked();
    let before = cx.capture_screenshot(window.into()).unwrap();
    let (width, height) = before.dimensions();
    assert_eq!(before.get_pixel(width / 4, height / 2).0, [255, 0, 0, 255]);
    assert_eq!(
        before.get_pixel(width * 3 / 4, height / 2).0,
        [0, 255, 0, 255]
    );
    let left_before = left_renders.get();
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
    })
    .unwrap();
    right.update(&mut cx, |tile, cx| {
        tile.key = 3;
        tile.color = [0, 0, 255, 255];
        cx.notify();
    });
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    assert_eq!(left_renders.get(), left_before, "left view must be reused");
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
    })
    .unwrap();
    right.update(&mut cx, |tile, cx| {
        tile.key = 7;
        cx.notify();
    });
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    assert_eq!(
        left_renders.get(),
        left_before,
        "left view must still be reused"
    );
    let after = cx.capture_screenshot(window.into()).unwrap();
    assert_eq!(
        after.get_pixel(width * 3 / 4, height / 2).0,
        [0, 0, 255, 255],
        "the changing view must have repainted"
    );
    assert_eq!(
        after.get_pixel(width / 4, height / 2).0,
        [255, 0, 0, 255],
        "the retained left image changed after eviction"
    );

    // One window can evict an image that is still displayed in another.
    let shared = Rc::new(Mutex::new(ImageCache::new(512 * 512 * 4)));
    let first = cx
        .open_window(gpui::size(px(20.), px(20.)), |_, cx| {
            cx.new(|_| Tile {
                cache: shared.clone(),
                key: 101,
                color: [255, 0, 0, 255],
                renders: Rc::new(Cell::new(0)),
            })
        })
        .unwrap();
    cx.run_until_parked();
    let red_id = shared.lock().unwrap().get(&101).unwrap().id;
    let second = cx
        .open_window(gpui::size(px(20.), px(20.)), |_, cx| {
            cx.new(|_| Tile {
                cache: shared.clone(),
                key: 102,
                color: [0, 255, 0, 255],
                renders: Rc::new(Cell::new(0)),
            })
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(second.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
    })
    .unwrap();
    let image = cx.capture_screenshot(first.into()).unwrap();
    assert_eq!(
        image.get_pixel(image.width() / 2, image.height() / 2).0,
        [255, 0, 0, 255]
    );
    RETIRED_IMAGES.with(|retired| assert!(retired.borrow().contains_key(&red_id)));
    cx.update_window(first.into(), |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    cx.update_window(second.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    RETIRED_IMAGES.with(|retired| assert!(!retired.borrow().contains_key(&red_id)));
}
