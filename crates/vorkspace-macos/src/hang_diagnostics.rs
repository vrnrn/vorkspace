//! Local, allowlisted post-hoc foreground timing records. No application payloads.
mod active_task;
mod storage;

use gpui::{
    App,
    profiler::hang::{
        HangIncident, HangMonitorConfig, HangMonitorPoll, HangMonitorPollReason, HangTrigger,
        SerializedHangContributor, SerializedHangIncident,
    },
};
use serde::Serialize;
use std::{
    io::{self, Write},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const THRESHOLD: Duration = Duration::from_millis(500);
const FRAME_BUDGET: Duration = Duration::from_millis(32);
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const MAX_CONTRIBUTORS: usize = 8;
const MAX_STRING_BYTES: usize = 160;
const MAX_RECORD_BYTES: usize = 32 * 1024;
const MAX_RECORDS_PER_POLL: usize = 8;

/// Called exactly once, at the beginning of the native application callback.
/// GPUI owns thread lifetime and the final worker flush (200ms shutdown budget).
pub fn start(cx: &mut App, path: PathBuf) {
    let callback = callback(cx, path);
    // Errors are deliberately silent: never expose paths/payloads or interrupt UI.
    let _ = cx.start_hang_monitor(config(), callback);
}

fn callback(cx: &App, path: PathBuf) -> impl FnMut(HangMonitorPoll) + Send + 'static + use<> {
    let startup = cx.background_executor().now();
    let run = Run::new();
    let mut active_task = active_task::Observer::new();
    let mut sink = Sink::new(path);
    move |poll: HangMonitorPoll| {
        if sink.disabled {
            return;
        }
        let active = active_task.poll(&run);
        let completed_limit = MAX_RECORDS_PER_POLL - usize::from(active.is_some());
        let elided = poll
            .incidents
            .iter()
            .filter(|i| qualifies(i))
            .count()
            .saturating_sub(completed_limit);
        if let Some(active) = active {
            sink.record(&active);
        }
        for incident in poll
            .incidents
            .iter()
            .filter(|i| qualifies(i))
            .take(completed_limit)
        {
            if sink.disabled {
                break;
            }
            let timing = SerializedHangIncident::convert(
                startup,
                incident,
                MAX_CONTRIBUTORS,
                poll.first_present_at,
            );
            let mut record = Record::new(&run, timing, elided);
            if let gpui::profiler::journal::IntervalBoundary::FrameSkipped(skipped) =
                incident.snapshot.boundary
            {
                use gpui::profiler::journal::FrameSkipReason;
                record.frame_skip_reason = Some(match skipped.reason {
                    FrameSkipReason::NoRenderNeeded => "no_render_needed",
                    FrameSkipReason::InactiveFrameRateLimit => "inactive_frame_rate_limit",
                    FrameSkipReason::ThermalFrameRateLimit => "thermal_frame_rate_limit",
                });
            }
            sink.record(&record);
        }
        if poll.reason == HangMonitorPollReason::Flush {
            sink.flush();
        }
    }
}

fn config() -> HangMonitorConfig {
    HangMonitorConfig {
        threshold: THRESHOLD,
        frame_budget: FRAME_BUDGET,
        interval: POLL_INTERVAL,
    }
}

fn qualifies(incident: &HangIncident) -> bool {
    // GPUI also reports ordinary missed frame budgets. Those must not grow logs.
    incident.trigger == HangTrigger::Threshold
        && incident
            .contributors
            .iter()
            .any(|event| event.duration() >= THRESHOLD)
}

#[derive(Serialize)]
struct Run {
    started_unix_ms: u64,
    pid: u32,
    version: &'static str,
}
impl Run {
    fn new() -> Self {
        Self {
            started_unix_ms: unix_ms(),
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

// Explicit schema: never serialize a GPUI object wholesale. New upstream fields
// require a deliberate privacy review before they can enter a local record.
#[derive(Serialize)]
struct Record<'a> {
    schema_version: u32,
    run: &'a Run,
    observed_unix_ms: u64,
    threshold_ms: u64,
    measurement_version: u32,
    phase: &'static str,
    during_active_use: bool,
    trigger: &'static str,
    start_ms: f64,
    active_ms: f64,
    stall_ms: f64,
    dirty_to_present_ms: Option<f64>,
    sealed_by: &'static str,
    frame_skip_reason: Option<&'static str>,
    busy_fraction: f64,
    event_count: usize,
    small_poll_count: u64,
    small_poll_total_ms: f64,
    dropped_events: u64,
    journal_discontinuous: bool,
    contributors_elided: usize,
    incidents_elided_in_poll: usize,
    contributors: Vec<Contributor>,
}
#[derive(Serialize)]
struct Contributor {
    kind: &'static str,
    start_ms: f64,
    duration_ms: f64,
    depth: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<Source>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    self_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    draw: Option<Draw>,
}
#[derive(Serialize)]
struct Source {
    file: String,
    line: u32,
    column: u32,
}
#[derive(Serialize)]
struct Draw {
    request_layout_ms: f64,
    prepaint_ms: f64,
    paint_ms: f64,
    dirty_to_draw_ms: Option<f64>,
    invalidations: u64,
}

impl<'a> Record<'a> {
    fn new(run: &'a Run, timing: SerializedHangIncident, elided: usize) -> Self {
        Self {
            schema_version: 1,
            run,
            observed_unix_ms: unix_ms(),
            threshold_ms: THRESHOLD.as_millis() as u64,
            measurement_version: timing.measurement_version,
            phase: timing.phase,
            during_active_use: timing.during_active_use,
            trigger: timing.trigger.as_str(),
            start_ms: timing.start_ms,
            active_ms: timing.active_ms,
            stall_ms: timing.stall_ms,
            dirty_to_present_ms: timing.dirty_to_present_ms,
            sealed_by: timing.sealed_by,
            frame_skip_reason: None,
            busy_fraction: timing.busy_fraction,
            event_count: timing.event_count,
            small_poll_count: timing.small_poll_count,
            small_poll_total_ms: timing.small_poll_total_ms,
            dropped_events: timing.dropped_events,
            journal_discontinuous: timing.journal_discontinuous,
            contributors_elided: timing.contributors_elided
                + timing.contributors.len().saturating_sub(MAX_CONTRIBUTORS),
            incidents_elided_in_poll: elided,
            contributors: timing
                .contributors
                .into_iter()
                .take(MAX_CONTRIBUTORS)
                .map(Contributor::from)
                .collect(),
        }
    }
}
impl From<SerializedHangContributor> for Contributor {
    fn from(value: SerializedHangContributor) -> Self {
        use SerializedHangContributor::*;
        let (kind, start_ms, duration_ms, depth) = match &value {
            TaskPoll {
                start_ms,
                duration_ms,
                depth,
                ..
            } => ("task_poll", *start_ms, *duration_ms, *depth),
            Action {
                start_ms,
                duration_ms,
                depth,
                ..
            } => ("action", *start_ms, *duration_ms, *depth),
            Input {
                start_ms,
                duration_ms,
                depth,
                ..
            } => ("input", *start_ms, *duration_ms, *depth),
            Draw {
                start_ms,
                duration_ms,
                depth,
                ..
            } => ("draw", *start_ms, *duration_ms, *depth),
            View {
                start_ms,
                duration_ms,
                depth,
                ..
            } => ("view", *start_ms, *duration_ms, *depth),
            Present {
                start_ms,
                duration_ms,
                depth,
                ..
            } => ("present", *start_ms, *duration_ms, *depth),
        };
        let mut result = Self {
            kind,
            start_ms,
            duration_ms,
            depth,
            name: None,
            source: None,
            phase: None,
            self_ms: None,
            draw: None,
        };
        match value {
            TaskPoll { location, .. } => {
                // Compile-time source only, with build-machine path prefixes removed.
                result.source = Some(Source {
                    file: source_file(location.file.as_ref()),
                    line: location.line,
                    column: location.column,
                });
            }
            Action { name, .. } => result.name = Some(bounded(name)),
            View {
                name,
                phase,
                self_ms,
                ..
            } => {
                result.name = Some(bounded(name));
                result.phase = Some(match phase {
                    "request_layout" => "request_layout",
                    "prepaint" => "prepaint",
                    "paint" => "paint",
                    _ => "unknown",
                });
                result.self_ms = Some(self_ms);
            }
            Draw {
                request_layout_ms,
                prepaint_ms,
                paint_ms,
                dirty_to_draw_ms,
                invalidations,
                ..
            } => {
                result.draw = Some(self::Draw {
                    request_layout_ms,
                    prepaint_ms,
                    paint_ms,
                    dirty_to_draw_ms,
                    invalidations,
                });
            }
            Input { input_kind, .. } => result.name = Some(bounded(safe_input_kind(input_kind))),
            _ => {}
        }
        result
    }
}
fn source_file(file: &str) -> String {
    let relative = file
        .strip_prefix("crates/")
        .or_else(|| file.strip_prefix("vendor/"))
        .or_else(|| file.rsplit_once("/crates/").map(|(_, tail)| tail))
        .or_else(|| file.rsplit_once("/vendor/").map(|(_, tail)| tail))
        .unwrap_or_else(|| file.rsplit('/').next().unwrap_or("unknown"));
    bounded(relative)
}
fn safe_input_kind(kind: &str) -> &'static str {
    match kind {
        "key_down" => "key_down",
        "key_up" => "key_up",
        "modifiers_changed" => "modifiers_changed",
        "mouse_down" => "mouse_down",
        "mouse_up" => "mouse_up",
        "mouse_move" => "mouse_move",
        "mouse_exited" => "mouse_exited",
        "mouse_pressure" => "mouse_pressure",
        "scroll_wheel" => "scroll_wheel",
        _ => "other",
    }
}
fn bounded(value: &str) -> String {
    let mut end = value.len().min(MAX_STRING_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

struct BoundedBuffer(Vec<u8>);
impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_RECORD_BYTES - 1 {
            return Err(io::Error::other("diagnostic record cap"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode(record: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut buffer = BoundedBuffer(Vec::with_capacity(MAX_RECORD_BYTES));
    serde_json::to_writer(&mut buffer, record)?;
    buffer.0.push(b'\n');
    Ok(buffer.0)
}

struct Sink {
    path: PathBuf,
    writer: Option<storage::Log>,
    disabled: bool,
}
impl Sink {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            writer: None,
            disabled: false,
        }
    }
    fn record(&mut self, record: &impl Serialize) {
        if self.disabled {
            return;
        }
        if self.write(record).is_err() {
            self.disable();
        }
    }
    fn write(&mut self, record: &impl Serialize) -> io::Result<()> {
        let bytes = encode(record)?;
        if self.writer.is_none() {
            self.writer = Some(storage::Log::open(&self.path)?);
        }
        if let Some(writer) = self.writer.as_mut() {
            writer.append(&bytes)?;
        }
        Ok(())
    }
    fn flush(&mut self) {
        if self
            .writer
            .as_mut()
            .is_some_and(|writer| writer.flush().is_err())
        {
            self.disable();
        }
    }
    fn disable(&mut self) {
        self.disabled = true;
        self.writer = None;
    }
}

#[cfg(test)]
mod tests;
