//! Observe an unfinished foreground poll without waiting for its journal event.
use super::{Run, Source, source_file, unix_ms};
use gpui::profiler::{ActiveTiming, GLOBAL_THREAD_TIMINGS};
use serde::Serialize;
use std::{thread::ThreadId, time::Duration};

const THRESHOLD: Duration = Duration::from_secs(5);

pub(super) struct Observer {
    foreground: ThreadId,
    reported: Option<ActiveTiming>,
}

impl Observer {
    pub(super) fn new() -> Self {
        Self {
            foreground: std::thread::current().id(),
            reported: None,
        }
    }

    pub(super) fn poll<'a>(&mut self, run: &'a Run) -> Option<Record<'a>> {
        let running = try_running(self.foreground)?;
        let elapsed = running.start.elapsed();
        self.admit(running, elapsed)
            .then(|| Record::new(run, running, elapsed))
    }

    fn admit(&mut self, running: ActiveTiming, elapsed: Duration) -> bool {
        if elapsed < THRESHOLD
            || self
                .reported
                .is_some_and(|reported| reported.start == running.start)
        {
            return false;
        }
        self.reported = Some(running);
        true
    }
}

fn try_running(thread: ThreadId) -> Option<ActiveTiming> {
    // Do not block the profiler, and never drop an upgraded Arc under its
    // registry lock: ThreadTimings::drop acquires that registry again.
    let weak = {
        let registry = GLOBAL_THREAD_TIMINGS.try_lock()?;
        registry
            .iter()
            .find(|entry| entry.thread_id == thread)?
            .timings
            .clone()
    };
    let timings = weak.upgrade()?;
    timings.try_lock()?.running
}

#[derive(Serialize)]
pub(super) struct Record<'a> {
    schema_version: u32,
    record_kind: &'static str,
    run: &'a Run,
    observed_unix_ms: u64,
    threshold_ms: u64,
    elapsed_ms: f64,
    source: Source,
}

impl<'a> Record<'a> {
    fn new(run: &'a Run, running: ActiveTiming, elapsed: Duration) -> Self {
        Self {
            schema_version: 1,
            record_kind: "active_task",
            run,
            observed_unix_ms: unix_ms(),
            threshold_ms: THRESHOLD.as_millis() as u64,
            elapsed_ms: elapsed.as_secs_f64() * 1000.,
            source: Source {
                file: source_file(running.location.file()),
                line: running.location.line(),
                column: running.location.column(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TasksIncluded, profiler::TaskTiming};

    fn running() -> ActiveTiming {
        let timing = TaskTiming::placeholder();
        ActiveTiming {
            location: std::panic::Location::caller(),
            spawned: timing.spawned,
            start: timing.start,
        }
    }

    #[test]
    fn reports_a_still_running_poll_once_and_admits_its_successor() {
        let mut observer = Observer::new();
        let first = running();
        assert!(!observer.admit(first, Duration::from_millis(4999)));
        assert!(observer.admit(first, THRESHOLD));
        assert!(!observer.admit(first, Duration::from_secs(120)));
        let successor = ActiveTiming {
            start: first.start + Duration::from_millis(1),
            ..first
        };
        assert!(!observer.admit(successor, Duration::ZERO));
        assert!(observer.admit(successor, THRESHOLD));
    }

    #[test]
    fn a_busy_profiler_registry_or_thread_is_skipped_without_waiting() {
        gpui::profiler::get_current_thread_task_timings(TasksIncluded::OnlyCompleted);
        let thread = std::thread::current().id();
        let weak = {
            let registry = GLOBAL_THREAD_TIMINGS.lock();
            let weak = registry
                .iter()
                .find(|entry| entry.thread_id == thread)
                .unwrap()
                .timings
                .clone();
            assert!(try_running(thread).is_none());
            weak
        };
        let timings = weak.upgrade().unwrap();
        let _guard = timings.lock();
        assert!(try_running(thread).is_none());
    }

    #[test]
    fn unfinished_task_records_use_the_existing_private_bounded_sink() {
        let root = tempfile::tempdir_in("/private/tmp").unwrap();
        let path = root.path().join("diagnostics");
        let run = Run::new();
        let record = Record::new(&run, running(), Duration::from_secs(10));
        let mut sink = super::super::Sink::new(path.clone());
        sink.record(&record);
        sink.flush();
        assert!(!sink.disabled);
        let bytes = std::fs::read(path.join("hang-0.jsonl")).unwrap();
        assert!(bytes.len() < super::super::MAX_RECORD_BYTES);
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["record_kind"], "active_task");
        assert_eq!(value["run"]["pid"], std::process::id());
        assert_eq!(value["elapsed_ms"], 10_000.);
        assert_eq!(value["threshold_ms"], 5000);
        assert_eq!(
            value["source"]["file"],
            "vorkspace-gpui/src/hang_diagnostics/active_task.rs"
        );
        assert!(value.get("sealed_by").is_none());
        assert!(value.get("during_active_use").is_none());
        assert_eq!(value.as_object().unwrap().len(), 7);
    }
}
