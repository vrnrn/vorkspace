//! 3D curved Bot avatars (`80%` spherical curvature, direct backing, `4` rings/cell
//! concentric texture at `60%` opacity) + sidebar cursor gaze tracking & textured pills.
use crate::clock::Instant;
use gpui::{
    AnyElement, Bounds, Corners, DispatchPhase, Pixels, Point, RenderImage, Window, canvas,
    prelude::*, px,
};
use image::Frame;
use std::{
    collections::HashMap,
    hash::Hash,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use vorkspace_core::{
    bot::Bot,
    project_appearance::{
        BADGE_SHAPES, BOT_FACES, BotAnimState, default_foreground_color, default_seed_for,
        normalize_badge_shape, normalize_bot_face, render_badge_or_bot_with_eyes,
    },
};

/// Origin of every avatar clock. Idle beats and status ticks are absolute
/// offsets from it, so avatars in different views wake at the same instants and
/// GPUI coalesces their notifications into one window frame.
#[cfg(not(test))]
static START: LazyLock<Instant> = LazyLock::new(Instant::now);

#[cfg(test)]
thread_local! {
    static TEST_START: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

fn avatar_start() -> Instant {
    #[cfg(not(test))]
    {
        *START
    }
    #[cfg(test)]
    {
        TEST_START.with(|start| match start.get() {
            Some(start) => start,
            None => {
                let now = Instant::now();
                start.set(Some(now));
                now
            }
        })
    }
}

#[cfg(test)]
pub(crate) fn reset_test_clock() {
    TEST_START.set(None);
}

/// Bound avatar motion to about 30 fps. Chat headers have their own rendering
/// owner, so decorative ticks never notify the conversation.
const AVATAR_FRAME: Duration = Duration::from_millis(33);

type PendingFrameKey = (gpui::AnyWindowHandle, gpui::EntityId);

/// The earliest admitted wake of one window and rendering owner.
struct PendingWake {
    generation: u64,
    deadline: Instant,
    /// Held so a sooner wake cancels this timer instead of leaving it to
    /// expire silently. `None` until the spawned task is attached.
    task: Option<gpui::Task<()>>,
}

thread_local! {
    static PENDING_FRAMES: std::cell::RefCell<HashMap<PendingFrameKey, PendingWake>> =
        Default::default();
    static FRAME_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

// Own admission inside the future, so dropping it before its first poll or at
// any await releases the slot. A cancelled repaint must never freeze a view.
struct PendingAvatarFrame {
    key: PendingFrameKey,
    generation: u64,
}
impl PendingAvatarFrame {
    /// Admit a wake unless an earlier or equal one is already pending. A sooner
    /// deadline supersedes and cancels a later one.
    fn acquire(key: PendingFrameKey, deadline: Instant) -> Option<Self> {
        let generation = FRAME_GENERATION.with(|counter| {
            counter.set(counter.get() + 1);
            counter.get()
        });
        let superseded = PENDING_FRAMES.with(|pending| {
            let mut pending = pending.borrow_mut();
            if pending
                .get(&key)
                .is_some_and(|wake| wake.deadline <= deadline)
            {
                return Err(());
            }
            let wake = PendingWake {
                generation,
                deadline,
                task: None,
            };
            Ok(pending.insert(key, wake))
        });
        // Cancelling drops the superseded future, whose guard re-enters the
        // map; it must run after the borrow above is released.
        let superseded = match superseded {
            Ok(wake) => wake,
            Err(()) => return None,
        };
        drop(superseded);
        Some(Self { key, generation })
    }
    fn is_current(&self) -> bool {
        PENDING_FRAMES.with(|pending| {
            pending
                .borrow()
                .get(&self.key)
                .is_some_and(|wake| wake.generation == self.generation)
        })
    }
    /// Let a later admission cancel `task`. If this admission was already
    /// superseded, the task is dropped (cancelled) outside the map borrow.
    fn attach_to(key: PendingFrameKey, generation: u64, task: gpui::Task<()>) {
        let stale = PENDING_FRAMES.with(|pending| match pending.borrow_mut().get_mut(&key) {
            Some(wake) if wake.generation == generation => wake.task.replace(task),
            _ => Some(task),
        });
        drop(stale);
    }
}
impl Drop for PendingAvatarFrame {
    fn drop(&mut self) {
        // The executor may drop cancelled tasks during thread-local teardown.
        let released = PENDING_FRAMES
            .try_with(|pending| {
                let mut pending = pending.borrow_mut();
                pending
                    .get(&self.key)
                    .is_some_and(|wake| wake.generation == self.generation)
                    .then(|| pending.remove(&self.key))
                    .flatten()
            })
            .ok()
            .flatten();
        // This guard lives inside the very task being released: detach rather
        // than cancel it from within its own poll.
        if let Some(task) = released.and_then(|wake| wake.task) {
            task.detach();
        }
    }
}

/// Wake the painting view when the avatar clock reaches `at` (an offset from
/// `START`). Every avatar painted by a view shares the earliest pending wake.
/// Window context validation prevents publication into a closed window.
fn request_avatar_frame(window: &Window, cx: &gpui::App, at: Duration) {
    let view = window.current_view();
    let deadline = avatar_start() + at;
    let Some(pending) = PendingAvatarFrame::acquire((window.window_handle(), view), deadline)
    else {
        return;
    };
    let delay = deadline.saturating_duration_since(Instant::now());
    let generation = pending.generation;
    let key = pending.key;
    let task = window.spawn(cx, async move |cx| {
        cx.background_executor().timer(delay).await;
        let current = pending.is_current();
        drop(pending);
        if current {
            let _ = cx.update(|_, cx| cx.notify(view));
        }
    });
    PendingAvatarFrame::attach_to(key, generation, task);
}

/// The next status tick on the shared avatar frame grid.
fn next_tick(elapsed: Duration) -> Duration {
    let frame = AVATAR_FRAME.as_millis() as u64;
    Duration::from_millis((elapsed.as_millis() as u64 / frame + 1) * frame)
}

/// Idle life is sparse: every beat, a Bot may glance somewhere new and/or
/// blink. Motion happens only in the first few hundred milliseconds of a beat
/// and the pose is held until the next beat that changes it, so an idle avatar
/// asks for about ten frames on an eventful beat and none in between. Beats are
/// window-wide; any number of idle avatars share the same frames.
const IDLE_BEAT_MS: u64 = 4_000;
const IDLE_GLANCE_MS: u64 = 300;
const IDLE_BLINK_MS: u64 = 200;
const IDLE_REST: (f32, f32) = (0.25, -0.18);
const IDLE_LOOKS: [(f32, f32); 5] = [
    (-0.38, -0.12),
    (0.52, -0.06),
    (-0.22, 0.2),
    (0.36, 0.24),
    (0.05, -0.36),
];

/// The look held during `beat` and whether it opens with a blink.
fn idle_beat(seed: u32, beat: u64) -> ((f32, f32), bool) {
    // SplitMix64 finalizer: decorrelates neighbouring seeds and beats.
    let mut h = ((seed as u64) << 32 ^ beat).wrapping_add(0x9e37_79b9_7f4a_7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    let look = if h.is_multiple_of(3) {
        IDLE_LOOKS[(h >> 8) as usize % IDLE_LOOKS.len()]
    } else {
        IDLE_REST
    };
    (look, (h >> 16).is_multiple_of(3))
}

fn idle_look_before(seed: u32, beat: u64) -> (f32, f32) {
    beat.checked_sub(1)
        .map_or(IDLE_REST, |previous| idle_beat(seed, previous).0)
}

/// How long `beat` moves the avatar, in milliseconds (zero for a quiet beat).
fn idle_motion_ms(seed: u32, beat: u64) -> u64 {
    let (look, blinks) = idle_beat(seed, beat);
    let glance = if look != idle_look_before(seed, beat) {
        IDLE_GLANCE_MS
    } else {
        0
    };
    glance.max(if blinks { IDLE_BLINK_MS } else { 0 })
}

struct IdleLife {
    look: (f32, f32),
    blink: f32,
    /// When the pose next changes, as an offset from `START`.
    next: Duration,
}

fn idle_life(seed: u32, elapsed: Duration) -> IdleLife {
    let ms = elapsed.as_millis() as u64;
    let (beat, local) = (ms / IDLE_BEAT_MS, ms % IDLE_BEAT_MS);
    let (to, blinks) = idle_beat(seed, beat);
    let from = idle_look_before(seed, beat);
    let look = if from != to && local < IDLE_GLANCE_MS {
        let u = local as f32 / IDLE_GLANCE_MS as f32;
        let e = u * u * (3. - 2. * u);
        (from.0 + (to.0 - from.0) * e, from.1 + (to.1 - from.1) * e)
    } else {
        to
    };
    let blink = if blinks && local < IDLE_BLINK_MS {
        let u = local as f32 / IDLE_BLINK_MS as f32;
        (u * std::f32::consts::PI).cos().abs().max(0.08)
    } else {
        1.
    };
    // While moving, tick; the first tick past the motion paints the settled
    // pose. Then sleep until the next beat that moves this avatar at all.
    let next = if local < idle_motion_ms(seed, beat) {
        next_tick(elapsed)
    } else {
        let beat = (beat + 1..beat + 64)
            .find(|&beat| idle_motion_ms(seed, beat) > 0)
            .unwrap_or(beat + 64);
        Duration::from_millis(beat * IDLE_BEAT_MS)
    };
    IdleLife { look, blink, next }
}

#[derive(Default)]
struct SidebarCursorState {
    rail_bounds: Option<Bounds<Pixels>>,
    mouse_pos: Option<Point<Pixels>>,
}
impl SidebarCursorState {
    fn track(&mut self, position: Point<Pixels>) -> bool {
        let previous = self.mouse_pos.replace(position);
        previous != Some(position)
            && self.rail_bounds.is_some_and(|rail| {
                rail.contains(&position) || previous.is_some_and(|pos| rail.contains(&pos))
            })
    }
}
type SharedSidebarCursor = std::rc::Rc<std::cell::RefCell<SidebarCursorState>>;
thread_local! {
    static SIDEBAR_CURSORS: std::cell::RefCell<HashMap<gpui::AnyWindowHandle, SharedSidebarCursor>> = Default::default();
}

fn sidebar_cursor(window: &Window) -> Option<SharedSidebarCursor> {
    SIDEBAR_CURSORS.with(|cursors| cursors.borrow().get(&window.window_handle()).cloned())
}

/// Record the sidebar rail bounds and track global mouse movement so Bot avatars
/// smoothly look toward the cursor whenever it enters the sidebar.
pub(crate) fn sidebar_cursor_tracker() -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let handle = window.window_handle();
            let cursor = window
                .use_keyed_state("sidebar-cursor-tracker", cx, |_, cx| {
                    let cursor =
                        std::rc::Rc::new(std::cell::RefCell::new(SidebarCursorState::default()));
                    SIDEBAR_CURSORS.with(|cursors| {
                        cursors.borrow_mut().insert(handle, cursor.clone());
                    });
                    cx.on_release(move |cursor: &mut SharedSidebarCursor, _| {
                        let _ = SIDEBAR_CURSORS.try_with(|cursors| {
                            let mut cursors = cursors.borrow_mut();
                            if cursors
                                .get(&handle)
                                .is_some_and(|current| std::rc::Rc::ptr_eq(current, cursor))
                            {
                                cursors.remove(&handle);
                            }
                        });
                    })
                    .detach();
                    cursor
                })
                .read(cx)
                .clone();
            cursor.borrow_mut().rail_bounds = Some(bounds);
            let view = window.current_view();
            window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture && cursor.borrow_mut().track(event.position) {
                    cx.notify(view);
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

thread_local! {
    // An evicted image may still appear in a cached GPUI view. Its keyed pins
    // live as long as that view's scene, including when GPUI replays it.
    static RETIRED_IMAGES: std::cell::RefCell<HashMap<gpui::ImageId, Arc<RenderImage>>> = Default::default();
}

struct SceneImagePin(Option<Arc<RenderImage>>);

fn pin_scene_image(window: &mut Window, cx: &mut gpui::App, image: &Arc<RenderImage>) {
    let handle = window.window_handle();
    let key = format!(
        "avatar-scene-image:{:?}:{:?}",
        window.current_view(),
        image.id
    );
    window.use_keyed_state(key, cx, |_, cx| {
        cx.on_release(move |pin: &mut SceneImagePin, cx| {
            let Some(image) = pin.0.take() else { return };
            let retired = RETIRED_IMAGES.with(|images| images.borrow().contains_key(&image.id));
            drop(image);
            if retired {
                // The owner may still be drawing; wait until it is returned to
                // App before asking for a frame in which to release the tile.
                cx.defer(move |cx| {
                    if handle
                        .update(cx, |_, window, _| schedule_retired_images(window))
                        .is_err()
                    {
                        release_retired_images(cx, None);
                    }
                });
            }
        })
        .detach();
        SceneImagePin(Some(image.clone()))
    });
}

fn release_retired_images(cx: &mut gpui::App, window: Option<&mut Window>) {
    let images = RETIRED_IMAGES.with(|retired| {
        let mut retired = retired.borrow_mut();
        let ready: Vec<_> = retired
            .iter()
            .filter_map(|(id, image)| (Arc::strong_count(image) == 1).then_some(*id))
            .collect();
        ready
            .into_iter()
            .filter_map(|id| retired.remove(&id))
            .collect::<Vec<_>>()
    });
    let mut window = window;
    for image in images {
        cx.drop_image(image, window.as_deref_mut());
    }
}

fn schedule_retired_images(window: &Window) {
    window.on_next_frame(|window, cx| release_retired_images(cx, Some(window)));
}

/// Eviction removes the CPU cache entry immediately. Atlas tiles are released
/// only after every retained scene has released its pin.
fn retire_images(window: &Window, images: Vec<Arc<RenderImage>>) {
    if images.is_empty() {
        return;
    }
    RETIRED_IMAGES.with(|retired| {
        let mut retired = retired.borrow_mut();
        for image in images {
            retired.insert(image.id, image);
        }
    });
    schedule_retired_images(window);
}

/// Rasterized images under a byte budget, least recently used evicted first.
/// Callers retire every returned image through `retire_images`.
struct ImageCache<K> {
    entries: HashMap<K, (Arc<RenderImage>, u64)>,
    bytes: usize,
    budget: usize,
    uses: u64,
}
impl<K: Hash + Eq> ImageCache<K> {
    fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            budget,
            uses: 0,
        }
    }

    fn get(&mut self, key: &K) -> Option<Arc<RenderImage>> {
        self.uses += 1;
        let (image, used) = self.entries.get_mut(key)?;
        *used = self.uses;
        Some(image.clone())
    }

    /// Insert `image` and return the evicted images. Eviction trims to three
    /// quarters of the budget so a stream of new poses sorts rarely. An image
    /// larger than the whole budget is painted once and returned for retirement
    /// rather than evicting the hot working set.
    #[must_use]
    fn insert(&mut self, key: K, image: Arc<RenderImage>) -> Vec<Arc<RenderImage>> {
        let bytes = image_bytes(&image);
        if bytes > self.budget {
            return vec![image];
        }
        self.uses += 1;
        let mut evicted = Vec::new();
        if let Some((previous, _)) = self.entries.insert(key, (image, self.uses)) {
            self.bytes -= image_bytes(&previous);
            evicted.push(previous);
        }
        self.bytes += bytes;
        if self.bytes > self.budget {
            let mut by_age: Vec<(u64, usize)> = self
                .entries
                .values()
                .map(|(image, used)| (*used, image_bytes(image)))
                .collect();
            by_age.sort_unstable();
            // Use stamps are unique: evict everything at or before `cutoff`.
            let mut cutoff = 0;
            for (used, bytes) in by_age {
                if self.bytes <= self.budget / 4 * 3 || used == self.uses {
                    break;
                }
                self.bytes -= bytes;
                cutoff = used;
            }
            self.entries.retain(|_, (image, used)| {
                let keep = *used > cutoff;
                if !keep {
                    evicted.push(image.clone());
                }
                keep
            });
        }
        evicted
    }
}

fn image_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count())
        .map(|frame| {
            let size = image.size(frame);
            size.width.0.max(0) as usize * size.height.0.max(0) as usize * 4
        })
        .sum()
}

/// Look up or rasterize an image in `cache`, retiring whatever it evicts.
fn cached_image<K: Hash + Eq>(
    cache: &Mutex<ImageCache<K>>,
    key: K,
    window: &mut Window,
    cx: &mut gpui::App,
    render: impl FnOnce() -> Arc<RenderImage>,
) -> Arc<RenderImage> {
    let cached = cache.lock().ok().and_then(|mut cache| cache.get(&key));
    let image = cached.unwrap_or_else(|| {
        // Rasterization never holds the shared lock.
        let image = render();
        if let Ok(mut cache) = cache.lock() {
            retire_images(window, cache.insert(key, image.clone()));
        }
        image
    });
    pin_scene_image(window, cx, &image);
    image
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BotLookConfig {
    pub shape: String,
    pub face: String,
    pub one_eye: bool,
    pub textured: bool,
    pub seed: u32,
    pub grid: u8,
    pub bg_color: u32,
    pub fg_color: u32,
}
impl BotLookConfig {
    pub fn from_bot(bot: &Bot) -> Self {
        if bot.is_minion() {
            return minion_look(bot, &bot.id);
        }
        Self {
            shape: bot.badge_shape().into(),
            face: bot.face_style().into(),
            one_eye: false,
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
            one_eye: false,
            textured: true,
            seed,
            grid: 4,
            bg_color: color,
            fg_color: default_foreground_color(color),
        }
    }
}

/// Geometry and pattern belong to the persistent team-member identity; only
/// the palette follows its parent. Separate salts keep the face, body and grid
/// choices independent, without storing appearance on the backend model.
pub(crate) fn minion_look(parent: &Bot, identity: &str) -> BotLookConfig {
    let choice = |part: &str| default_seed_for(&format!("minion-{part}:{identity}")) as usize;
    BotLookConfig {
        shape: BADGE_SHAPES[choice("body") % BADGE_SHAPES.len()].into(),
        face: BOT_FACES[choice("face") % BOT_FACES.len()].into(),
        one_eye: true,
        textured: true,
        seed: default_seed_for(identity),
        grid: 2 + (choice("grid") % 7) as u8,
        bg_color: parent.color,
        fg_color: parent.resolved_fg_color(),
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct FrameCacheKey {
    is_bot: bool,
    shape: String,
    face: String,
    one_eye: bool,
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

/// About 3,000 sidebar poses or 250 header poses: every idle pose of a full
/// sidebar stays warm, while working motion and pointer gaze recycle.
const FRAME_CACHE_BYTES: usize = 16 * 1024 * 1024;
static FRAME_CACHE: LazyLock<Mutex<ImageCache<FrameCacheKey>>> =
    LazyLock::new(|| Mutex::new(ImageCache::new(FRAME_CACHE_BYTES)));

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
        one_eye: is_bot && look.one_eye,
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

fn render_frame(key: &FrameCacheKey, look: &BotLookConfig, anim: BotAnimState) -> Arc<RenderImage> {
    to_render_image(render_badge_or_bot_with_eyes(
        key.is_bot,
        &key.shape,
        &key.face,
        key.needs_you,
        look.textured,
        look.seed,
        look.grid,
        look.bg_color,
        look.fg_color,
        key.pixel_side,
        key.one_eye,
        anim,
    ))
}

pub(crate) fn cached_badge_or_bot_image(
    is_bot: bool,
    look: &BotLookConfig,
    needs_you: bool,
    pixel_side: u32,
    anim: BotAnimState,
    window: &mut Window,
    cx: &mut gpui::App,
) -> Arc<RenderImage> {
    let key = frame_cache_key(is_bot, look, needs_you, pixel_side, anim);
    cached_image(&FRAME_CACHE, key.clone(), window, cx, || {
        render_frame(&key, look, anim)
    })
}

/// Render a 30% opacity full-button concentric texture / color highlight pill for
/// selected Project and Channel rows in the sidebar.
fn compute_bot_anim(
    seed: u32,
    status: &str,
    bounds: Bounds<Pixels>,
    animate: bool,
    window: &Window,
) -> (BotAnimState, Option<Duration>) {
    let gaze = if animate {
        sidebar_cursor(window).and_then(|cursor| {
            let cursor = cursor.borrow();
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
    bot_anim_at(seed, status, animate, avatar_start().elapsed(), gaze)
}

// Pointer input belongs to the sidebar only. The header shares idle life and
// status motion without reading or registering any pointer state.
fn header_anim_at(
    seed: u32,
    status: &str,
    animate: bool,
    elapsed: Duration,
) -> (BotAnimState, Option<Duration>) {
    bot_anim_at(seed, status, animate, elapsed, None)
}

/// The pose at `elapsed` and when it next changes (`None`: only input or a
/// status change can change it). Pointer gaze is published by input events.
fn bot_anim_at(
    seed: u32,
    status: &str,
    animate: bool,
    elapsed: Duration,
    gaze: Option<(f32, f32)>,
) -> (BotAnimState, Option<Duration>) {
    let rest = BotAnimState {
        yaw: IDLE_REST.0,
        pitch: IDLE_REST.1,
        wide: if status == "needs-you" { 1.14 } else { 1.0 },
        ..Default::default()
    };
    if !animate {
        return (rest, None);
    }
    if !matches!(status, "working" | "needs-you") {
        let life = idle_life(seed, elapsed);
        let (yaw, pitch) = gaze.unwrap_or(life.look);
        let anim = BotAnimState {
            yaw,
            pitch,
            blink: life.blink,
            ..rest
        };
        return (anim, Some(life.next));
    }

    let now = elapsed.as_secs_f32();
    let t = now + (seed % 97) as f32 * 0.17;
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
        } else {
            anim.yaw = (t * 6.0).sin() * 0.05;
            anim.pitch = -0.12;
            anim.wide = 1.14;
            anim.hop_y = -(t * 3.2).sin().abs() * 0.035;
        }
    } else if status == "working" {
        anim.tex_rot = (now * 14.0).rem_euclid(360.0);
    }

    (anim, Some(next_tick(elapsed)))
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

fn schedule(window: &Window, cx: &gpui::App, next: Option<Duration>) {
    if let Some(at) = next
        && !crate::clock::suppress_endless_animation_frames()
    {
        request_avatar_frame(window, cx, at);
    }
}

pub(crate) fn bot_avatar(bot: &Bot, status: &str, side: f32, still: bool) -> AnyElement {
    avatar_with_look(BotLookConfig::from_bot(bot), status, side, still)
}

/// The same cached renderer and bounded clocks as ordinary Bot avatars.
/// Callers supply the member's persistent ID, never its mutable name or role.
pub(crate) fn minion_avatar(
    parent: &Bot,
    identity: &str,
    status: &str,
    side: f32,
    still: bool,
) -> AnyElement {
    avatar_with_look(minion_look(parent, identity), status, side, still)
}

/// Chat identity has explicitly no pointer gaze. Unlike `avatar_with_look` and
/// `settings_preview`, this path never consults a cursor or installs a listener.
pub(super) fn header_avatar(
    look: BotLookConfig,
    status: &str,
    side: f32,
    visible: bool,
) -> AnyElement {
    let status = status.to_owned();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let animate = visible
                && window.is_window_active()
                && bounds.intersects(&window.content_mask().bounds);
            let (anim, next) =
                header_anim_at(look.seed, &status, animate, avatar_start().elapsed());
            schedule(window, cx, next);
            let pixel_side = (bounds.size.width.as_f32() * window.scale_factor())
                .ceil()
                .clamp(28., 128.) as u32;
            let image = cached_badge_or_bot_image(
                true,
                &look,
                status == "needs-you",
                pixel_side,
                anim,
                window,
                cx,
            );
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
    elapsed: Duration,
) -> (BotAnimState, Option<Duration>) {
    let mut anim = BotAnimState::default();
    let center = bounds.center();
    anim.yaw = ((mouse.x - center.x).as_f32() / 240.).clamp(-0.78, 0.78);
    anim.pitch = ((mouse.y - center.y).as_f32() / 240.).clamp(-0.68, 0.68);
    anim.crossed_eyes = crossed_eyes;
    if crossed_eyes {
        return (anim, None);
    }
    // Gaze follows the pointer; only the idle blinks need the clock.
    let life = idle_life(seed, elapsed);
    anim.blink = life.blink;
    (anim, Some(life.next))
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
    let handle = window.window_handle();
    let frame = window.use_keyed_state(
        gpui::SharedString::from(format!("bot-settings-preview:{}", bot.id)),
        cx,
        |_, cx| {
            // The retained frame owns its atlas tile.
            cx.on_release(
                move |frame: &mut Option<(FrameCacheKey, Arc<RenderImage>)>, cx| {
                    if let Some((_, image)) = frame.take() {
                        cx.defer(move |cx| {
                            let mut image = Some(image);
                            if handle
                                .update(cx, |_, window, _| {
                                    retire_images(window, vec![image.take().unwrap()]);
                                })
                                .is_err()
                            {
                                if let Some(image) = image.take() {
                                    RETIRED_IMAGES.with(|retired| {
                                        retired.borrow_mut().insert(image.id, image);
                                    });
                                }
                                release_retired_images(cx, None);
                            }
                        });
                    }
                },
            )
            .detach();
            None::<(FrameCacheKey, Arc<RenderImage>)>
        },
    );
    canvas(
        |_, _, _| {},
        move |bounds, _, window: &mut Window, cx| {
            let mouse = window.mouse_position();
            let crossed_eyes = delete_anchor.get().is_some_and(|b| b.contains(&mouse));
            let animate = window.is_window_active();
            let (anim, next) = preview_anim(
                look.seed,
                bounds,
                mouse,
                crossed_eyes,
                avatar_start().elapsed(),
            );
            if animate {
                schedule(window, cx, next);
            }
            // Capture the drawing owner now: input dispatch has no rendering context.
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
                let image = render_frame(&key, &look, anim);
                let previous = frame.update(cx, |frame, _| frame.replace((key, image.clone())));
                retire_images(
                    window,
                    previous.into_iter().map(|(_, image)| image).collect(),
                );
                image
            });
            pin_scene_image(window, cx, &image);
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
            let sidebar_hovered = sidebar_cursor(window).is_some_and(|c| {
                let c = c.borrow();
                c.rail_bounds.zip(c.mouse_pos).is_some_and(|(rail, pos)| {
                    rail.contains(&pos) && rail.contains(&bounds.center())
                })
            });
            let animate = window.is_window_active()
                && bounds.intersects(&window.content_mask().bounds)
                && (!still || status == "working" || sidebar_hovered);
            let (anim, next) = compute_bot_anim(look.seed, &status, bounds, animate, window);
            schedule(window, cx, next);
            let image = cached_badge_or_bot_image(
                true,
                &look,
                status == "needs-you",
                pixel_side,
                anim,
                window,
                cx,
            );
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

static SWATCH_CACHE: LazyLock<Mutex<ImageCache<SwatchCacheKey>>> =
    LazyLock::new(|| Mutex::new(ImageCache::new(1024 * 1024)));

/// Render a crisp, anti-aliased circular color swatch (solid `bg` when `fg` is `None`,
/// or vertically split left=`bg` / right=`fg` when `fg` is `Some`).
pub(crate) fn color_swatch_circle(bg: u32, fg: Option<u32>, side: f32) -> AnyElement {
    let pixel_side = ((side * 2.0).round() as u32).clamp(24, 64);
    let key = SwatchCacheKey { bg, fg, pixel_side };
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let image = cached_image(&SWATCH_CACHE, key, window, cx, || {
                render_swatch(bg, fg, pixel_side)
            });
            let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
        },
    )
    .size(px(side))
    .flex_shrink_0()
    .into_any_element()
}

fn render_swatch(bg: u32, fg: Option<u32>, pixel_side: u32) -> Arc<RenderImage> {
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
    to_render_image(buf)
}

#[cfg(test)]
mod header_tests {
    use super::*;

    #[gpui::test]
    fn avatar_clock_origin_follows_the_bound_executor(cx: &mut gpui::TestAppContext) {
        crate::clock::use_test_clock(cx);
        let first_origin = avatar_start();
        cx.background_executor
            .advance_clock(Duration::from_secs(100));
        crate::clock::use_test_clock(cx);
        assert_eq!(
            avatar_start(),
            first_origin,
            "rebinding the same executor must preserve its origin"
        );
        assert_eq!(avatar_start().elapsed(), Duration::from_secs(100));
        let other = gpui::TestAppContext::build(gpui::TestDispatcher::new(42), None);
        crate::clock::use_test_clock(&other);
        assert_eq!(
            avatar_start(),
            Instant::now(),
            "a new executor must get a fresh origin"
        );
        assert_eq!(avatar_start().elapsed(), Duration::ZERO);
    }

    #[gpui::test]
    fn pending_avatar_frame_keeps_the_earliest_wake_and_releases_on_cancellation(
        cx: &mut gpui::TestAppContext,
    ) {
        let first = cx.new(|_| ());
        let second = cx.new(|_| ());
        let open = |cx: &mut gpui::App| -> gpui::AnyWindowHandle {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| AvatarFrameTestView(None))
            })
            .unwrap()
            .into()
        };
        let window: gpui::AnyWindowHandle = cx.update(open);
        let other_window: gpui::AnyWindowHandle = cx.update(open);
        let id = (window, first.entity_id());
        let at = |ms| avatar_start() + Duration::from_millis(ms);
        let pending = PendingAvatarFrame::acquire(id, at(100)).unwrap();
        // An equal or later wake is already covered.
        assert!(PendingAvatarFrame::acquire(id, at(100)).is_none());
        assert!(PendingAvatarFrame::acquire(id, at(4_000)).is_none());
        // A sooner wake supersedes; the superseded owner can no longer
        // notify, and its cancellation must not release its successor.
        let sooner = PendingAvatarFrame::acquire(id, at(33)).unwrap();
        assert!(!pending.is_current() && sooner.is_current());
        drop(pending);
        assert!(PendingAvatarFrame::acquire(id, at(66)).is_none());
        // Other owning views and windows are independently admitted.
        let other = PendingAvatarFrame::acquire((window, second.entity_id()), at(100)).unwrap();
        let another_window =
            PendingAvatarFrame::acquire((other_window, first.entity_id()), at(100)).unwrap();
        let unpolled = async move {
            let _pending = sooner;
            std::future::pending::<()>().await;
        };
        drop(unpolled);
        let replacement = PendingAvatarFrame::acquire(id, at(4_000)).unwrap();
        // Also cancel after a poll reached an await boundary.
        let mut polled = Box::pin(async move {
            let _pending = replacement;
            std::future::pending::<()>().await;
        });
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(polled.as_mut(), &mut context).is_pending());
        drop(polled);
        assert!(PendingAvatarFrame::acquire(id, at(4_000)).is_some());
        drop(other);
        drop(another_window);
        PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
    }

    #[gpui::test]
    fn avatar_frame_coalesces_supersedes_and_releases_after_timer(cx: &mut gpui::TestAppContext) {
        crate::clock::use_test_clock(cx);
        let renders = std::rc::Rc::new(std::cell::Cell::new(0));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| AvatarFrameTestView(Some(vec![AVATAR_FRAME * 30, AVATAR_FRAME])))
            })
            .unwrap()
        });
        let counter = renders.clone();
        let view = window.root(cx).unwrap();
        let _observer =
            cx.update(|cx| cx.observe(&view, move |_, _| counter.set(counter.get() + 1)));
        cx.refresh().unwrap();
        cx.run_until_parked();
        PENDING_FRAMES.with(|pending| assert_eq!(pending.borrow().len(), 1));
        // The sooner of the two wakes notifies once; the superseded timer is
        // silent when it later expires.
        cx.background_executor.advance_clock(AVATAR_FRAME * 2);
        cx.run_until_parked();
        assert_eq!(renders.get(), 1);
        PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
        cx.background_executor.advance_clock(AVATAR_FRAME * 30);
        cx.run_until_parked();
        assert_eq!(renders.get(), 1);
        window
            .update(cx, |view, _, cx| {
                view.0 = Some(vec![AVATAR_FRAME]);
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        cx.run_until_parked();
        PENDING_FRAMES.with(|pending| assert_eq!(pending.borrow().len(), 1));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.background_executor.advance_clock(AVATAR_FRAME * 2);
        cx.run_until_parked();
        PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
    }

    #[gpui::test]
    fn due_avatar_wake_is_not_replaced_by_a_later_deadline(cx: &mut gpui::TestAppContext) {
        crate::clock::use_test_clock(cx);
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| AvatarFrameTestView(None))
            })
            .unwrap()
        });
        let view = window.root(cx).unwrap();
        let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = notifications.clone();
        let _observer =
            cx.update(|cx| cx.observe(&view, move |_, _| counter.set(counter.get() + 1)));
        let key = (window.into(), view.entity_id());
        cx.update(|cx| {
            let pending = PendingAvatarFrame::acquire(key, Instant::now() - AVATAR_FRAME).unwrap();
            let generation = pending.generation;
            let view_id = view.entity_id();
            // Model a due timer queued but not yet polled by the executor.
            let task = cx.spawn(async move |cx| {
                let current = pending.is_current();
                drop(pending);
                if current {
                    cx.update(|cx| cx.notify(view_id));
                }
            });
            PendingAvatarFrame::attach_to(key, generation, task);
            assert!(PendingAvatarFrame::acquire(key, Instant::now() + AVATAR_FRAME * 5).is_none());
        });
        cx.run_until_parked();
        assert_eq!(notifications.get(), 1);
        PENDING_FRAMES.with(|pending| assert!(pending.borrow().is_empty()));
    }

    /// Requests each listed delay (relative to now) on its next paint.
    struct AvatarFrameTestView(Option<Vec<Duration>>);
    impl gpui::Render for AvatarFrameTestView {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let delays = self.0.take().unwrap_or_default();
            gpui::canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    for delay in &delays {
                        request_avatar_frame(window, cx, avatar_start().elapsed() + *delay);
                    }
                },
            )
            .size(px(20.))
        }
    }

    #[test]
    fn header_modes_share_motion_without_pointer_gaze() {
        let look = BotLookConfig::from_legacy("robot", 0x3b82f6);
        let at = Duration::from_secs(6);
        for status in ["idle", "working", "needs-you"] {
            let (header, next) = header_anim_at(look.seed, status, true, at);
            let (autonomous, _) = bot_anim_at(look.seed, status, true, at, None);
            let (tracking, _) = bot_anim_at(look.seed, status, true, at, Some((0.78, 0.68)));
            let key = |anim| frame_cache_key(true, &look, status == "needs-you", 128, anim);
            assert!(next.is_some_and(|next| next > at));
            assert!(key(header) == key(autonomous));
            assert!(key(header) != key(tracking));
            assert_eq!(tracking.yaw, 0.78);
            assert_eq!(tracking.pitch, 0.68);
            assert!(!header.crossed_eyes);
        }
        let (working, next) = header_anim_at(look.seed, "working", true, at);
        let (attention, _) = header_anim_at(look.seed, "needs-you", true, at);
        assert_eq!(working.tex_rot, 84.);
        assert_eq!(next, Some(Duration::from_millis(6_006)));
        assert_eq!(attention.wide, 1.14);
    }

    #[test]
    fn header_hidden_state_is_frozen_without_frame_requests() {
        let look = BotLookConfig::from_legacy("cat", 0xe86b5f);
        for status in ["idle", "working", "needs-you"] {
            let (first, next) = header_anim_at(look.seed, status, false, Duration::ZERO);
            let (later, later_next) =
                header_anim_at(look.seed, status, false, Duration::from_secs(900));
            assert!(next.is_none() && later_next.is_none());
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
        // Between beats the preview holds a still, open-eyed pose.
        let quiet = Duration::from_millis(IDLE_BEAT_MS * 3 + 2_000);
        let preview = |mouse, crossed| preview_anim(look.seed, bounds, mouse, crossed, quiet);
        let (left, _) = preview(gpui::point(px(0.), px(0.)), false);
        let (right, _) = preview(gpui::point(px(600.), px(400.)), false);
        assert!(left.yaw < 0. && left.pitch < 0.);
        assert!(right.yaw > 0. && right.pitch > 0.);
        let (crossed, crossed_next) = preview(bounds.center(), true);
        let (restored, restored_next) = preview(bounds.center(), false);
        assert!(crossed.crossed_eyes && !restored.crossed_eyes);
        // Crossed eyes never blink; the restored face resumes idle blinks.
        assert!(crossed_next.is_none());
        assert!(restored_next.is_some_and(|next| next > quiet));
        assert!(
            frame_cache_key(true, &look, false, 256, crossed)
                != frame_cache_key(true, &look, false, 256, restored)
        );
        assert_eq!(restored.yaw, 0.);
        assert_eq!(restored.pitch, 0.);
        assert_eq!(restored.blink, 1.);
        assert_eq!(look.face, "visor");
    }
}

#[cfg(test)]
mod idle_tests;

#[cfg(test)]
mod minion_tests;
