//! 3D curved Bot avatars (`80%` spherical curvature, direct backing, `4` rings/cell
//! concentric texture at `60%` opacity) + sidebar cursor gaze tracking & textured pills.
use crate::clock::Instant;
use gpui::{
    AnyElement, Bounds, Corners, DispatchPhase, ObjectFit, Pixels, Point, RenderImage, Window,
    canvas, img, prelude::*, px,
};
use image::Frame;
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};
use vorkspace_core::{
    bot::Bot,
    project_appearance::{
        BotAnimState, default_foreground_color, default_seed_for, normalize_badge_shape,
        normalize_bot_face, render_badge_or_bot, render_textured_pill,
    },
};

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Autonomous avatar motion is slow, but each frame re-renders the avatar's
/// owning view (a whole chat or sidebar). Bound that work to about 30 fps.
const AVATAR_FRAME: std::time::Duration = std::time::Duration::from_millis(33);

type PendingFrameKey = (gpui::AnyWindowHandle, gpui::EntityId);
thread_local! {
    static PENDING_FRAMES: std::cell::RefCell<std::collections::HashSet<PendingFrameKey>> =
        Default::default();
}

// Own admission inside the future, so dropping it before its first poll or at
// any await releases the slot. A cancelled repaint must never freeze a view.
struct PendingAvatarFrame(PendingFrameKey);
impl PendingAvatarFrame {
    fn acquire(view: PendingFrameKey) -> Option<Self> {
        PENDING_FRAMES.with(|pending| pending.borrow_mut().insert(view).then(|| Self(view)))
    }
}
impl Drop for PendingAvatarFrame {
    fn drop(&mut self) {
        // The executor may drop cancelled tasks during thread-local teardown.
        let _ = PENDING_FRAMES.try_with(|pending| {
            pending.borrow_mut().remove(&self.0);
        });
    }
}

/// Every avatar painted by a view shares one pending repaint. Window context
/// validation prevents publication into a closed window.
fn request_avatar_frame(window: &Window, cx: &gpui::App) {
    let view = window.current_view();
    let Some(pending) = PendingAvatarFrame::acquire((window.window_handle(), view)) else {
        return;
    };
    window
        .spawn(cx, async move |cx| {
            cx.background_executor().timer(AVATAR_FRAME).await;
            drop(pending);
            let _ = cx.update(|_, cx| cx.notify(view));
        })
        .detach();
}

#[derive(Default)]
struct SidebarCursorState {
    rail_bounds: Option<Bounds<Pixels>>,
    mouse_pos: Option<Point<Pixels>>,
}
static SIDEBAR_CURSOR: LazyLock<Mutex<SidebarCursorState>> =
    LazyLock::new(|| Mutex::new(SidebarCursorState::default()));

/// Record the sidebar rail bounds and track global mouse movement so Bot avatars
/// smoothly look toward the cursor whenever it enters the sidebar.
pub(crate) fn sidebar_cursor_tracker() -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if let Ok(mut state) = SIDEBAR_CURSOR.lock() {
                state.rail_bounds = Some(bounds);
            }
            window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, _| {
                if phase == DispatchPhase::Capture
                    && let Ok(mut state) = SIDEBAR_CURSOR.lock()
                {
                    state.mouse_pos = Some(event.position);
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

pub(crate) fn to_render_image(mut pixels: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![Frame::new(pixels)]))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BotLookConfig {
    pub shape: String,
    pub face: String,
    pub textured: bool,
    pub seed: u32,
    pub grid: u8,
    pub bg_color: u32,
    pub fg_color: u32,
}
impl BotLookConfig {
    pub fn from_bot(bot: &Bot) -> Self {
        Self {
            shape: bot.badge_shape().into(),
            face: bot.face_style().into(),
            textured: bot.textured,
            seed: bot.resolved_seed(),
            grid: bot.resolved_grid(),
            bg_color: bot.color,
            fg_color: bot.resolved_fg_color(),
        }
    }
    pub fn from_legacy(mascot: &str, color: u32) -> Self {
        let seed = default_seed_for(&format!("{mascot}:{color:06x}"));
        Self {
            shape: "squircle".into(),
            face: normalize_bot_face(mascot).into(),
            textured: true,
            seed,
            grid: 4,
            bg_color: color,
            fg_color: default_foreground_color(color),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct FrameCacheKey {
    is_bot: bool,
    shape: String,
    face: String,
    needs_you: bool,
    textured: bool,
    seed: u32,
    grid: u8,
    bg_color: u32,
    fg_color: u32,
    pixel_side: u32,
    yaw_q: i16,
    pitch_q: i16,
    blink_q: i16,
    squash_x_q: i16,
    squash_y_q: i16,
    hop_y_q: i16,
    wide_q: i16,
    tex_rot_q: i16,
    crossed_eyes: bool,
}

static FRAME_CACHE: LazyLock<Mutex<HashMap<FrameCacheKey, Arc<RenderImage>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, PartialEq, Eq, Hash)]
struct PillCacheKey {
    seed: u32,
    grid: u8,
    textured: bool,
    bg_color: u32,
    fg_color: u32,
    width: u32,
    height: u32,
}

static PILL_CACHE: LazyLock<Mutex<HashMap<PillCacheKey, Arc<RenderImage>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn frame_cache_key(
    is_bot: bool,
    look: &BotLookConfig,
    needs_you: bool,
    pixel_side: u32,
    anim: BotAnimState,
) -> FrameCacheKey {
    FrameCacheKey {
        is_bot,
        shape: normalize_badge_shape(&look.shape).into(),
        face: normalize_bot_face(&look.face).into(),
        needs_you,
        textured: look.textured,
        seed: look.seed,
        grid: look.grid,
        bg_color: look.bg_color,
        fg_color: look.fg_color,
        pixel_side,
        yaw_q: (anim.yaw * 40.0).round() as i16,
        pitch_q: (anim.pitch * 40.0).round() as i16,
        blink_q: (anim.blink * 25.0).round() as i16,
        squash_x_q: (anim.squash_x * 100.0).round() as i16,
        squash_y_q: (anim.squash_y * 100.0).round() as i16,
        hop_y_q: (anim.hop_y * 100.0).round() as i16,
        wide_q: (anim.wide * 50.0).round() as i16,
        tex_rot_q: (anim.tex_rot * 0.5).round() as i16,
        crossed_eyes: anim.crossed_eyes,
    }
}

pub(crate) fn cached_badge_or_bot_image(
    is_bot: bool,
    look: &BotLookConfig,
    needs_you: bool,
    pixel_side: u32,
    anim: BotAnimState,
) -> Arc<RenderImage> {
    let key = frame_cache_key(is_bot, look, needs_you, pixel_side, anim);
    if let Ok(cache) = FRAME_CACHE.lock()
        && let Some(img) = cache.get(&key)
    {
        return img.clone();
    }
    let rendered = to_render_image(render_badge_or_bot(
        is_bot,
        &key.shape,
        &key.face,
        needs_you,
        look.textured,
        look.seed,
        look.grid,
        look.bg_color,
        look.fg_color,
        pixel_side,
        anim,
    ));
    if let Ok(mut cache) = FRAME_CACHE.lock() {
        if cache.len() > 512 {
            cache.clear();
        }
        cache.insert(key, rendered.clone());
    }
    rendered
}

/// Render a 30% opacity full-button concentric texture / color highlight pill for
/// selected Project and Channel rows in the sidebar.
pub(crate) fn sidebar_selection_pill(
    seed: u32,
    grid: u8,
    textured: bool,
    bg_color: u32,
    fg_color: u32,
) -> AnyElement {
    let key = PillCacheKey {
        seed,
        grid: grid.clamp(2, 8),
        textured,
        bg_color,
        fg_color,
        width: 360,
        height: 64,
    };
    let image = if let Ok(cache) = PILL_CACHE.lock()
        && let Some(found) = cache.get(&key)
    {
        found.clone()
    } else {
        let rendered = to_render_image(render_textured_pill(
            key.seed,
            key.grid,
            key.textured,
            key.bg_color,
            key.fg_color,
            key.width,
            key.height,
        ));
        if let Ok(mut cache) = PILL_CACHE.lock() {
            if cache.len() > 128 {
                cache.clear();
            }
            cache.insert(key, rendered.clone());
        }
        rendered
    };
    img(image)
        .absolute()
        .inset_0()
        .size_full()
        .rounded_md()
        .object_fit(ObjectFit::Fill)
        .into_any_element()
}

fn blink_at(t: f32) -> f32 {
    let cycle = (t * 0.22).rem_euclid(1.0);
    if cycle > 0.94 {
        let u = (cycle - 0.94) / 0.06;
        (u * std::f32::consts::PI).cos().abs().max(0.08)
    } else {
        1.
    }
}

fn compute_bot_anim(
    seed: u32,
    status: &str,
    bounds: Bounds<Pixels>,
    animate: bool,
) -> (BotAnimState, bool) {
    let gaze = if animate {
        SIDEBAR_CURSOR.lock().ok().and_then(|cursor| {
            let (rail, mouse) = cursor.rail_bounds.zip(cursor.mouse_pos)?;
            (rail.contains(&mouse) && rail.contains(&bounds.center())).then(|| {
                (
                    ((mouse.x - bounds.center().x).as_f32() / 140.0).clamp(-0.78, 0.78),
                    ((mouse.y - bounds.center().y).as_f32() / 140.0).clamp(-0.68, 0.68),
                )
            })
        })
    } else {
        None
    };
    bot_anim_at(seed, status, animate, START.elapsed().as_secs_f32(), gaze)
}

// Pointer input belongs to the sidebar only. The header shares these three
// autonomous animations without reading or registering any pointer state.
fn header_anim_at(seed: u32, status: &str, animate: bool, now: f32) -> (BotAnimState, bool) {
    bot_anim_at(seed, status, animate, now, None)
}

fn bot_anim_at(
    seed: u32,
    status: &str,
    animate: bool,
    now: f32,
    gaze: Option<(f32, f32)>,
) -> (BotAnimState, bool) {
    let offset = (seed % 97) as f32 * 0.17;
    if !animate {
        return (
            BotAnimState {
                yaw: 0.25,
                pitch: -0.18,
                blink: 1.0,
                squash_x: 1.0,
                squash_y: 1.0,
                hop_y: 0.0,
                wide: if status == "needs-you" { 1.14 } else { 1.0 },
                tex_rot: 0.0,
                crossed_eyes: false,
            },
            false,
        );
    }

    let t = now + offset;
    let mut anim = BotAnimState::default();

    if status != "needs-you" {
        anim.blink = blink_at(t);
    }

    if let Some((yaw, pitch)) = gaze {
        anim.yaw = yaw;
        anim.pitch = pitch;
    }

    if gaze.is_none() {
        if status == "working" {
            // Working: Rhythmic Nod + Half-Speed Texture Flow (14 deg/sec)
            let pulse = (t * 5.2).sin();
            anim.yaw = (t * 1.3).sin() * 0.28;
            anim.pitch = pulse * 0.28;
            anim.squash_x = 1.0 + pulse * 0.045;
            anim.squash_y = 1.0 - pulse * 0.045;
            anim.hop_y = -pulse.abs() * 0.025;
            anim.tex_rot = (now * 14.0).rem_euclid(360.0);
        } else if status == "needs-you" {
            anim.yaw = (t * 6.0).sin() * 0.05;
            anim.pitch = -0.12;
            anim.wide = 1.14;
            anim.hop_y = -(t * 3.2).sin().abs() * 0.035;
        } else {
            // Auto 3D Gaze (idle)
            let s1 = (t * 0.75).sin() + 0.45 * (t * 1.9).sin();
            let s2 = (t * 0.6).cos() - 0.35 * (t * 1.4).sin();
            anim.yaw = (s1 * 0.38).clamp(-0.68, 0.68);
            anim.pitch = (s2 * 0.30 - 0.08).clamp(-0.55, 0.55);
        }
    } else if status == "working" {
        anim.tex_rot = (now * 14.0).rem_euclid(360.0);
    }

    (anim, true)
}

pub(crate) fn bot_avatar(bot: &Bot, status: &str, side: f32, still: bool) -> AnyElement {
    avatar_with_look(BotLookConfig::from_bot(bot), status, side, still)
}

/// Chat identity has explicitly no pointer gaze. Unlike `avatar_with_look` and
/// `settings_preview`, this path never consults a cursor or installs a listener.
pub(super) fn header_avatar(bot: &Bot, status: &str, side: f32, visible: bool) -> AnyElement {
    let look = BotLookConfig::from_bot(bot);
    let status = status.to_owned();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let animate = visible
                && window.is_window_active()
                && bounds.intersects(&window.content_mask().bounds);
            let (anim, needs_frame) =
                header_anim_at(look.seed, &status, animate, START.elapsed().as_secs_f32());
            if needs_frame && !crate::clock::suppress_endless_animation_frames() {
                request_avatar_frame(window, cx);
            }
            let pixel_side = (bounds.size.width.as_f32() * window.scale_factor())
                .ceil()
                .clamp(28., 128.) as u32;
            let image =
                cached_badge_or_bot_image(true, &look, status == "needs-you", pixel_side, anim);
            let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
        },
    )
    .size(px(side))
    .flex_shrink_0()
    .into_any_element()
}

fn preview_anim(
    seed: u32,
    bounds: Bounds<Pixels>,
    mouse: Point<Pixels>,
    crossed_eyes: bool,
) -> (BotAnimState, bool) {
    let mut anim = BotAnimState::default();
    let center = bounds.center();
    anim.yaw = ((mouse.x - center.x).as_f32() / 240.).clamp(-0.78, 0.78);
    anim.pitch = ((mouse.y - center.y).as_f32() / 240.).clamp(-0.68, 0.68);
    anim.crossed_eyes = crossed_eyes;
    if !crossed_eyes {
        anim.blink = blink_at(START.elapsed().as_secs_f32() + (seed % 97) as f32 * 0.17);
    }
    (anim, true)
}

/// A large preview follows this window's pointer rather than the sidebar's
/// shared cursor. Retain only its last frame: high-resolution animation frames
/// must not accumulate in the sidebar image cache.
pub(super) fn settings_preview(
    bot: &Bot,
    side: f32,
    delete_anchor: crate::popover::AnchorSlot,
    window: &mut Window,
    cx: &mut gpui::App,
) -> AnyElement {
    let look = BotLookConfig::from_bot(bot);
    let frame = window.use_keyed_state(
        gpui::SharedString::from(format!("bot-settings-preview:{}", bot.id)),
        cx,
        |_, _| None::<(FrameCacheKey, Arc<RenderImage>)>,
    );
    canvas(
        |_, _, _| {},
        move |bounds, _, window: &mut Window, cx| {
            let mouse = window.mouse_position();
            let crossed_eyes = delete_anchor.get().is_some_and(|b| b.contains(&mouse));
            let (anim, needs_frame) = preview_anim(look.seed, bounds, mouse, crossed_eyes);
            if needs_frame && !crate::clock::suppress_endless_animation_frames() {
                window.request_animation_frame();
            }
            // Capture the drawing owner now: input dispatch has no rendering
            // context, so request_animation_frame cannot be called there.
            let view = window.current_view();
            window.on_mouse_event(move |_: &gpui::MouseMoveEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture {
                    cx.notify(view);
                }
            });
            let pixel_side = (bounds.size.width.as_f32() * window.scale_factor())
                .ceil()
                .clamp(128., 512.) as u32;
            let key = frame_cache_key(true, &look, false, pixel_side, anim);
            let cached = frame
                .read(cx)
                .as_ref()
                .filter(|(previous, _)| previous == &key)
                .map(|(_, image)| image.clone());
            let image = cached.unwrap_or_else(|| {
                let image = to_render_image(render_badge_or_bot(
                    true,
                    &look.shape,
                    &look.face,
                    false,
                    look.textured,
                    look.seed,
                    look.grid,
                    look.bg_color,
                    look.fg_color,
                    pixel_side,
                    anim,
                ));
                frame.update(cx, |frame, _| *frame = Some((key, image.clone())));
                image
            });
            let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
        },
    )
    .size(px(side))
    .flex_shrink_0()
    .into_any_element()
}

pub(crate) fn avatar_with_look(
    look: BotLookConfig,
    status: &str,
    side: f32,
    still: bool,
) -> AnyElement {
    let status = status.to_string();
    let pixel_side = ((side * 2.0).round() as u32).clamp(28, 128);
    canvas(
        |_, _, _| {},
        move |bounds, _, window: &mut Window, cx| {
            let sidebar_hovered = SIDEBAR_CURSOR.lock().ok().is_some_and(|c| {
                c.rail_bounds.zip(c.mouse_pos).is_some_and(|(rail, pos)| {
                    rail.contains(&pos) && rail.contains(&bounds.center())
                })
            });
            let animate = !still || status == "working" || sidebar_hovered;
            let (anim, needs_frame) = compute_bot_anim(look.seed, &status, bounds, animate);
            if needs_frame && !crate::clock::suppress_endless_animation_frames() {
                request_avatar_frame(window, cx);
            }
            let image =
                cached_badge_or_bot_image(true, &look, status == "needs-you", pixel_side, anim);
            let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
        },
    )
    .size(px(side))
    .flex_shrink_0()
    .into_any_element()
}

pub(crate) fn avatar(name: &str, color: u32, status: &str, side: f32, still: bool) -> AnyElement {
    avatar_with_look(BotLookConfig::from_legacy(name, color), status, side, still)
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct SwatchCacheKey {
    bg: u32,
    fg: Option<u32>,
    pixel_side: u32,
}

static SWATCH_CACHE: LazyLock<Mutex<HashMap<SwatchCacheKey, Arc<RenderImage>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Render a crisp, anti-aliased circular color swatch (solid `bg` when `fg` is `None`,
/// or vertically split left=`bg` / right=`fg` when `fg` is `Some`).
pub(crate) fn color_swatch_circle(bg: u32, fg: Option<u32>, side: f32) -> AnyElement {
    let pixel_side = ((side * 2.0).round() as u32).clamp(24, 64);
    let key = SwatchCacheKey { bg, fg, pixel_side };
    let image = if let Ok(cache) = SWATCH_CACHE.lock()
        && let Some(found) = cache.get(&key)
    {
        found.clone()
    } else {
        let mut buf = image::RgbaImage::new(pixel_side, pixel_side);
        let center = pixel_side as f32 * 0.5;
        let radius = center - 0.75;
        let bg_rgb = (
            ((bg >> 16) & 0xff) as f32,
            ((bg >> 8) & 0xff) as f32,
            (bg & 0xff) as f32,
        );
        let fg_rgb = fg
            .map(|c| {
                (
                    ((c >> 16) & 0xff) as f32,
                    ((c >> 8) & 0xff) as f32,
                    (c & 0xff) as f32,
                )
            })
            .unwrap_or(bg_rgb);
        for py in 0..pixel_side {
            let dy = py as f32 + 0.5 - center;
            for px_x in 0..pixel_side {
                let dx = px_x as f32 + 0.5 - center;
                let dist = (dx * dx + dy * dy).sqrt();
                let cov = (radius + 0.5 - dist).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let t = if fg.is_some() {
                    (dx + 0.5).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let r = (bg_rgb.0 * (1.0 - t) + fg_rgb.0 * t).round() as u8;
                let g = (bg_rgb.1 * (1.0 - t) + fg_rgb.1 * t).round() as u8;
                let b = (bg_rgb.2 * (1.0 - t) + fg_rgb.2 * t).round() as u8;
                let a = (cov * 255.0).round() as u8;
                buf.put_pixel(px_x, py, image::Rgba([r, g, b, a]));
            }
        }
        let rendered = to_render_image(buf);
        if let Ok(mut cache) = SWATCH_CACHE.lock() {
            if cache.len() > 128 {
                cache.clear();
            }
            cache.insert(key, rendered.clone());
        }
        rendered
    };
    img(image)
        .size(px(side))
        .object_fit(ObjectFit::Contain)
        .flex_shrink_0()
        .into_any_element()
}

#[cfg(test)]
mod header_tests {
    use super::*;

    #[gpui::test]
    fn pending_avatar_frame_releases_admission_on_cancellation(cx: &mut gpui::TestAppContext) {
        let first = cx.new(|_| ());
        let second = cx.new(|_| ());
        let open = |cx: &mut gpui::App| -> gpui::AnyWindowHandle {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| AvatarFrameTestView(false))
            })
            .unwrap()
            .into()
        };
        let window: gpui::AnyWindowHandle = cx.update(open);
        let other_window: gpui::AnyWindowHandle = cx.update(open);
        let id = (window, first.entity_id());
        let pending = PendingAvatarFrame::acquire(id).unwrap();
        assert!(PendingAvatarFrame::acquire(id).is_none());
        assert!(PendingAvatarFrame::acquire(id).is_none());
        // Another owning view is independently admitted.
        let other = PendingAvatarFrame::acquire((window, second.entity_id())).unwrap();
        let another_window =
            PendingAvatarFrame::acquire((other_window, first.entity_id())).unwrap();
        let unpolled = async move {
            let _pending = pending;
            std::future::pending::<()>().await;
        };
        drop(unpolled);
        let replacement = PendingAvatarFrame::acquire(id).unwrap();
        // Also cancel after a poll reached an await boundary.
        let mut polled = Box::pin(async move {
            let _pending = replacement;
            std::future::pending::<()>().await;
        });
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(polled.as_mut(), &mut context).is_pending());
        drop(polled);
        assert!(PendingAvatarFrame::acquire(id).is_some());
        drop(other);
        drop(another_window);
        PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
    }

    #[gpui::test]
    fn avatar_frame_coalesces_and_releases_after_timer(cx: &mut gpui::TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| AvatarFrameTestView(true))
            })
            .unwrap()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        PENDING_FRAMES.with(|pending| assert_eq!(pending.borrow().len(), 1));
        cx.background_executor.advance_clock(AVATAR_FRAME);
        cx.run_until_parked();
        PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
        window
            .update(cx, |view, _, cx| {
                view.0 = true;
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        PENDING_FRAMES.with(|pending| assert_eq!(pending.borrow().len(), 1));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.background_executor.advance_clock(AVATAR_FRAME);
        cx.run_until_parked();
        PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
    }

    struct AvatarFrameTestView(bool);
    impl gpui::Render for AvatarFrameTestView {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let schedule = std::mem::take(&mut self.0);
            gpui::canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    if schedule {
                        request_avatar_frame(window, cx);
                        request_avatar_frame(window, cx);
                    }
                },
            )
            .size(px(20.))
        }
    }

    #[test]
    fn header_modes_share_autonomous_animation_without_pointer_gaze() {
        let look = BotLookConfig::from_legacy("robot", 0x3b82f6);
        for status in ["idle", "working", "needs-you"] {
            let (header, moving) = header_anim_at(look.seed, status, true, 6.);
            let (autonomous, _) = bot_anim_at(look.seed, status, true, 6., None);
            let (tracking, _) = bot_anim_at(look.seed, status, true, 6., Some((0.78, 0.68)));
            let key = |anim| frame_cache_key(true, &look, status == "needs-you", 128, anim);
            assert!(moving);
            assert!(key(header) == key(autonomous));
            assert!(key(header) != key(tracking));
            assert_eq!(tracking.yaw, 0.78);
            assert_eq!(tracking.pitch, 0.68);
            assert!(!header.crossed_eyes);
        }
        let (working, _) = header_anim_at(look.seed, "working", true, 6.);
        let (attention, _) = header_anim_at(look.seed, "needs-you", true, 6.);
        assert_eq!(working.tex_rot, 84.);
        assert_eq!(attention.wide, 1.14);
    }

    #[test]
    fn header_hidden_state_is_frozen_without_frame_requests() {
        let look = BotLookConfig::from_legacy("cat", 0xe86b5f);
        for status in ["idle", "working", "needs-you"] {
            let (first, moving) = header_anim_at(look.seed, status, false, 0.);
            let (later, later_moving) = header_anim_at(look.seed, status, false, 900.);
            assert!(!moving && !later_moving);
            assert!(
                frame_cache_key(true, &look, status == "needs-you", 128, first)
                    == frame_cache_key(true, &look, status == "needs-you", 128, later)
            );
            assert_eq!(first.blink, 1.);
            assert_eq!(first.hop_y, 0.);
            assert_eq!(first.tex_rot, 0.);
        }
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn settings_preview_tracks_both_axes_and_restores_eyes_after_delete_hover() {
        let bounds = Bounds::new(
            gpui::point(px(200.), px(100.)),
            gpui::size(px(128.), px(128.)),
        );
        let look = BotLookConfig::from_legacy("robot", 0x3b82f6);
        let (left, _) = preview_anim(look.seed, bounds, gpui::point(px(0.), px(0.)), false);
        let (right, _) = preview_anim(look.seed, bounds, gpui::point(px(600.), px(400.)), false);
        assert!(left.yaw < 0. && left.pitch < 0.);
        assert!(right.yaw > 0. && right.pitch > 0.);
        let (crossed, _) = preview_anim(look.seed, bounds, bounds.center(), true);
        let (restored, _) = preview_anim(look.seed, bounds, bounds.center(), false);
        assert!(crossed.crossed_eyes && !restored.crossed_eyes);
        assert!(
            frame_cache_key(true, &look, false, 256, crossed)
                != frame_cache_key(true, &look, false, 256, restored)
        );
        assert_eq!(restored.yaw, 0.);
        assert_eq!(restored.pitch, 0.);
        assert_eq!(look.face, "visor");
    }
}
