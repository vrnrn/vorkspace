//! Opt-in, bounded tracing used by the isolated macOS cursor fixture.
//!
//! No cursor trace code is compiled unless `test-support` is enabled. Callers must
//! record only framework identifiers, event names, cursor styles, and geometry;
//! this module rejects control characters and caps every event payload.

use std::{
    cell::Cell,
    collections::{HashMap, VecDeque},
    fmt,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

const EVENT_CAPACITY: usize = 4096;
const DETAIL_LIMIT: usize = 240;
const MAPPING_CAPACITY: usize = 64;

static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static NEXT_FRAME: AtomicU64 = AtomicU64::new(1);
static TRACE: OnceLock<Mutex<TraceState>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct Event {
    pub sequence: u64,
    pub monotonic_ns: u128,
    pub gpui_window: u64,
    pub native_window: u64,
    pub frame: u64,
    pub kind: &'static str,
    pub detail: String,
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.sequence,
            self.monotonic_ns,
            self.gpui_window,
            self.native_window,
            self.frame,
            self.kind,
            self.detail
        )
    }
}

struct TraceState {
    started: Instant,
    events: VecDeque<Event>,
    dropped: u64,
    latest_frame: HashMap<u64, u64>,
    native_to_gpui: HashMap<u64, u64>,
}

impl Default for TraceState {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            events: VecDeque::with_capacity(EVENT_CAPACITY),
            dropped: 0,
            latest_frame: HashMap::new(),
            native_to_gpui: HashMap::new(),
        }
    }
}

fn trace() -> &'static Mutex<TraceState> {
    TRACE.get_or_init(|| Mutex::new(TraceState::default()))
}

#[derive(Clone, Copy)]
struct Context {
    gpui_window: u64,
    frame: u64,
}

thread_local! {
    static CURRENT_CONTEXT: Cell<Option<Context>> = const { Cell::new(None) };
}

pub struct Scope {
    previous: Option<Context>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT_CONTEXT.with(|context| context.set(self.previous));
    }
}

/// Starts a frame identity and makes it available to platform calls made during
/// the synchronous draw. The returned guard restores any enclosing context.
pub fn frame_scope(gpui_window: u64) -> Scope {
    let frame = NEXT_FRAME.fetch_add(1, Ordering::Relaxed);
    let previous = CURRENT_CONTEXT.with(|context| {
        let previous = context.get();
        context.set(Some(Context { gpui_window, frame }));
        previous
    });
    {
        let mut state = trace()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.latest_frame.len() >= MAPPING_CAPACITY
            && !state.latest_frame.contains_key(&gpui_window)
        {
            if let Some(first) = state.latest_frame.keys().next().copied() {
                state.latest_frame.remove(&first);
            }
        }
        state.latest_frame.insert(gpui_window, frame);
    }
    record_current("gpui.frame.begin", "");
    Scope { previous }
}

/// Associates a native fixture window with the GPUI window that created it.
pub fn associate_native_window(native_window: u64, gpui_window: u64) {
    let mut state = trace()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.native_to_gpui.len() >= MAPPING_CAPACITY
        && !state.native_to_gpui.contains_key(&native_window)
    {
        if let Some(first) = state.native_to_gpui.keys().next().copied() {
            state.native_to_gpui.remove(&first);
        }
    }
    state.native_to_gpui.insert(native_window, gpui_window);
}

/// Associates a native callback with its GPUI window and most recent frame.
pub fn native_scope(native_window: u64) -> Scope {
    let (gpui_window, frame) = {
        let state = trace()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let gpui_window = state
            .native_to_gpui
            .get(&native_window)
            .copied()
            .unwrap_or(0);
        let frame = state.latest_frame.get(&gpui_window).copied().unwrap_or(0);
        (gpui_window, frame)
    };
    let previous = CURRENT_CONTEXT.with(|context| {
        let previous = context.get();
        context.set((gpui_window != 0).then_some(Context { gpui_window, frame }));
        previous
    });
    Scope { previous }
}

pub fn record_current(kind: &'static str, detail: impl AsRef<str>) {
    let context = CURRENT_CONTEXT.with(Cell::get);
    record(
        context.map_or(0, |context| context.gpui_window),
        0,
        context.map_or(0, |context| context.frame),
        kind,
        detail.as_ref(),
    );
}

pub fn record_window(gpui_window: u64, kind: &'static str, detail: impl AsRef<str>) {
    let frame = trace()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .latest_frame
        .get(&gpui_window)
        .copied()
        .unwrap_or(0);
    record(gpui_window, 0, frame, kind, detail.as_ref());
}

pub fn record_native(native_window: u64, kind: &'static str, detail: impl AsRef<str>) {
    let (gpui_window, frame) = CURRENT_CONTEXT.with(|context| {
        context.get().map_or_else(
            || {
                let state = trace()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let gpui_window = state
                    .native_to_gpui
                    .get(&native_window)
                    .copied()
                    .unwrap_or(0);
                let frame = state.latest_frame.get(&gpui_window).copied().unwrap_or(0);
                (gpui_window, frame)
            },
            |context| (context.gpui_window, context.frame),
        )
    });
    record(gpui_window, native_window, frame, kind, detail.as_ref());
}

fn record(gpui_window: u64, native_window: u64, frame: u64, kind: &'static str, detail: &str) {
    let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut state = trace()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let monotonic_ns = state.started.elapsed().as_nanos();
    let detail = detail
        .chars()
        .filter(|character| !character.is_control())
        .take(DETAIL_LIMIT)
        .collect();
    if state.events.len() == EVENT_CAPACITY {
        state.events.pop_front();
        state.dropped = state.dropped.saturating_add(1);
    }
    state.events.push_back(Event {
        sequence,
        monotonic_ns,
        gpui_window,
        native_window,
        frame,
        kind,
        detail,
    });
}

/// Returns the bounded trace and the number of older records evicted from its ring.
pub fn snapshot() -> (Vec<Event>, u64) {
    let state = trace()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    (state.events.iter().cloned().collect(), state.dropped)
}

/// Clears fixture history without resetting process-wide sequence numbers.
pub fn clear() {
    let mut state = trace()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.started = Instant::now();
    state.events.clear();
    state.dropped = 0;
}
