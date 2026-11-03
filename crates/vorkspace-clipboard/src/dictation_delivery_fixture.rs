//! Debug-only public-audio exercise of the Service and its real worker, with a
//! memory destination standing in for native paste. Never accesses a field.
use super::*;
use std::{cell::RefCell, path::Path, rc::Rc};

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum FinishAction {
    Deliver,
    Cancel,
    Edit,
    Focus,
}

#[derive(serde::Deserialize)]
struct FixtureCase {
    wav: PathBuf,
    expected: String,
    #[serde(default)]
    expected_live: Option<String>,
    #[serde(default)]
    automatic: bool,
    #[serde(default)]
    expect_no_suffix: bool,
    action: FinishAction,
}

struct Clip {
    samples: Vec<f32>,
    expected: Option<String>,
    expected_live: Option<String>,
    automatic: bool,
    expect_no_suffix: bool,
    action: FinishAction,
}

pub(super) struct Sink {
    pub epoch: u64,
    pub next: u64,
    pub text: String,
    pub valid: bool,
    pub forced_busy: bool,
    pub busy_until: Option<Instant>,
    pub blocked: usize,
    pub deltas: Vec<String>,
}
impl Sink {
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            next: 0,
            text: "Existing: ".into(),
            valid: true,
            forced_busy: false,
            busy_until: None,
            blocked: 0,
            deltas: Vec::new(),
        }
    }
    pub fn paste(&mut self, text: &str, epoch: u64, delivery: u64) -> i32 {
        // Match the native order: target identity is checked even while the
        // previous paste still owns the clipboard read window.
        if !self.valid || epoch != self.epoch || delivery != self.next {
            return 9;
        }
        if self.forced_busy || self.busy_until.is_some_and(|until| Instant::now() < until) {
            self.blocked += 1;
            return 12;
        }
        self.text.push_str(text);
        self.deltas.push(text.to_owned());
        self.next += 1;
        self.busy_until = Some(Instant::now() + Duration::from_millis(800));
        0
    }
}

fn pcm(path: &Path) -> Vec<f32> {
    assert!(path.is_absolute());
    let wav = hound::WavReader::open(path).expect("public/generated WAV");
    assert_eq!(wav.spec().sample_rate, 16000);
    assert_eq!(wav.spec().channels, 1);
    assert_eq!(wav.spec().bits_per_sample, 16);
    assert_eq!(wav.spec().sample_format, hound::SampleFormat::Int);
    assert!(wav.duration() > 0 && wav.duration() as usize <= worker::MAX_SAMPLES);
    let samples: Vec<_> = wav
        .into_samples::<i16>()
        .map(|v| v.unwrap() as f32 / 32768.)
        .collect();
    assert!(samples.len() <= worker::MAX_SAMPLES);
    samples
}

pub fn run(
    lock: PathBuf,
    model: PathBuf,
    question: PathBuf,
    statement: PathBuf,
    selected: ModelId,
) {
    assert!(lock.is_absolute() && model.is_absolute());
    let clips = [question, statement]
        .iter()
        .map(|path| Clip {
            samples: pcm(path),
            expected: None,
            expected_live: None,
            automatic: false,
            expect_no_suffix: false,
            action: FinishAction::Deliver,
        })
        .collect();
    run_clips(lock, model, clips, selected);
}

/// An explicit public/generated fixture matrix. A case can invalidate the
/// memory destination after Stop while the final suffix waits for the simulated
/// pasteboard read window; no native field or clipboard is accessed.
pub fn run_matrix(lock: PathBuf, model: PathBuf, manifest: &Path) {
    assert!(lock.is_absolute() && model.is_absolute() && manifest.is_absolute());
    let cases: Vec<FixtureCase> =
        serde_json::from_slice(&std::fs::read(manifest).expect("fixture manifest"))
            .expect("fixture cases");
    assert!(!cases.is_empty() && cases.len() <= 32);
    let clips = cases
        .into_iter()
        .map(|case| Clip {
            samples: pcm(&case.wav),
            expected: Some(case.expected),
            expected_live: case.expected_live,
            automatic: case.automatic,
            expect_no_suffix: case.expect_no_suffix,
            action: case.action,
        })
        .collect();
    run_clips(lock, model, clips, ModelId::UnifiedEnglish);
}

fn run_clips(lock: PathBuf, model: PathBuf, clips: Vec<Clip>, selected: ModelId) {
    let exit = Rc::new(std::cell::Cell::new(1));
    let result_code = exit.clone();
    gpui::Application::with_platform(Rc::new(gpui_macos::MacPlatform::new(false)))
        .with_windowing(gpui::WindowingRequest::Headless)
        .run(move |cx| {
            cx.set_global(crate::Helper::default());
            let service = cx.new(|_| {
                let mut s = Service::new(Ok(model));
                s.public_audio_fixture = true;
                s.fixture_engine_lock = Some(lock);
                s.initializing = false;
                s.attempt.enabled = true;
                s.selected = selected;
                s
            });
            service.update(cx, |s, cx| s.prepare_worker(cx));
            cx.spawn(async move |cx| {
                let outcome = exercise(&service, &clips, cx).await;
                service.update(cx, |s, cx| { s.attempt.enabled = false; s.cancel(cx); s.retire_worker(); });
                while service.read_with(cx, |s, _| s.worker.running) {
                    cx.background_executor().timer(Duration::from_millis(20)).await;
                }
                match outcome {
                    Ok(()) => { println!("fixture joined=true loaded_models={} microphone=false real_field=false", worker::model_loads()); result_code.set(0); }
                    Err(e) => eprintln!("fixture failure: {e}"),
                }
                cx.update(|cx| cx.quit());
            }).detach();
        });
    std::process::exit(exit.get());
}

async fn exercise(
    service: &Entity<Service>,
    clips: &[Clip],
    cx: &mut gpui::AsyncApp,
) -> Result<(), String> {
    let start = Instant::now();
    while !service.read_with(cx, |s, _| s.ready) {
        if start.elapsed() > Duration::from_secs(120) {
            return Err("worker did not prepare".into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    for (index, clip) in clips.iter().enumerate() {
        let epoch = service
            .update(cx, |s, _| s.attempt.begin())
            .ok_or("admission refused")?;
        let sink = Rc::new(RefCell::new(Sink::new(epoch)));
        service.update(cx, |s, _| {
            s.fixture_delivery = Some(sink.clone());
            s.begin_turn(epoch)
        })?;
        while service.read_with(cx, |s, _| s.attempt.phase) == Phase::Starting {
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
        }
        let start = Instant::now();
        for (i, chunk) in clip.samples.chunks(1600).enumerate() {
            let deadline = start + Duration::from_millis((i as u64 + 1) * 100);
            if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                cx.background_executor().timer(wait).await;
            }
            service.read_with(cx, |s, _| s.feed_audio(chunk))?;
            service.update(cx, |s, cx| s.deliver(epoch, cx));
        }
        let live = sink.borrow().text.clone();
        if live == "Existing: " {
            return Err(format!("turn {} had no live delivery", index + 1));
        }
        if clip
            .expected_live
            .as_ref()
            .is_some_and(|expected| live != format!("Existing: {expected}"))
        {
            return Err(format!(
                "turn {} unexpected live delivery: {live:?}",
                index + 1
            ));
        }
        sink.borrow_mut().forced_busy = true;
        let stopped_at = Instant::now();
        service.update(cx, |s, _| {
            if s.attempt.stop() != Some(epoch) {
                return Err("stop refused".into());
            }
            s.finish_input(if clip.automatic {
                worker::FinishReason::Automatic
            } else {
                worker::FinishReason::UserStop
            })
        })?;
        let deadline = Instant::now() + Duration::from_secs(15);
        while service.read_with(cx, |s, _| s.finished.is_none() && s.turn.is_some()) {
            if Instant::now() > deadline {
                return Err("no final worker receipt".into());
            }
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
        }
        let stop_to_worker_done_ms = stopped_at.elapsed().as_millis();
        let (pending, delivered, complete, ready) = service.read_with(cx, |s, _| {
            (
                s.pending_text.clone(),
                s.delivered.text().to_owned(),
                s.turn.is_none(),
                s.attempt.phase == Phase::Idle && s.status == "Ready · ⌘, to start and stop",
            )
        });
        if clip.expect_no_suffix {
            if !matches!(clip.action, FinishAction::Deliver)
                || !complete
                || !ready
                || pending != delivered
            {
                return Err("expected successful finalization without a suffix".into());
            }
        } else if complete
            || !pending.starts_with(&delivered)
            || pending == delivered
            || sink.borrow().blocked == 0
        {
            return Err("final suffix did not wait behind read window".into());
        }
        if clip
            .expected
            .as_ref()
            .is_some_and(|expected| *expected != pending)
        {
            return Err(format!(
                "turn {} unexpected transcript: {pending:?}",
                index + 1
            ));
        }
        let action = match clip.action {
            FinishAction::Deliver => "deliver",
            FinishAction::Cancel => {
                service.update(cx, |s, cx| s.cancel(cx));
                "cancel"
            }
            FinishAction::Edit => {
                sink.borrow_mut().text.push_str(" user edit");
                sink.borrow_mut().valid = false;
                "edit"
            }
            FinishAction::Focus => {
                sink.borrow_mut().valid = false;
                "focus"
            }
        };
        if matches!(clip.action, FinishAction::Edit | FinishAction::Focus) {
            // Destination validation precedes the read-window check, even
            // while the previous paste is still marked busy.
            let blocked = sink.borrow().blocked;
            service.update(cx, |s, cx| s.deliver(epoch, cx));
            if service.read_with(cx, |s, _| s.turn.is_some()) || sink.borrow().blocked != blocked {
                return Err("destination change was hidden by the read window".into());
            }
        }
        sink.borrow_mut().forced_busy = false;
        while service.read_with(cx, |s, _| s.turn.is_some()) {
            if Instant::now() > deadline {
                return Err("delivery did not finish".into());
            }
            service.update(cx, |s, cx| s.deliver(epoch, cx));
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
        }
        let final_text = sink.borrow().text.clone();
        let deltas = sink.borrow().deltas.clone();
        let expected_field = match clip.action {
            FinishAction::Deliver => format!("Existing: {pending}"),
            FinishAction::Edit => format!("Existing: {delivered} user edit"),
            FinishAction::Cancel | FinishAction::Focus => format!("Existing: {delivered}"),
        };
        let expected_deltas = match clip.action {
            FinishAction::Deliver => &pending,
            _ => &delivered,
        };
        if final_text != expected_field || deltas.concat() != *expected_deltas {
            return Err("delivery replayed or misplaced text".into());
        }
        // A delayed delivery callback or stale event cannot revive a retired
        // attempt after the destination changed or the user cancelled.
        service.update(cx, |s, cx| {
            s.deliver(epoch, cx);
            s.worker_event(
                s.worker.generation,
                s.selected,
                Event::Committed(epoch, pending.clone()),
                cx,
            );
            s.deliver(epoch, cx);
        });
        if sink.borrow().text != final_text || sink.borrow().deltas != deltas {
            return Err("retired attempt inserted later text".into());
        }
        println!(
            "turn={} action={action} automatic={} no_suffix={} live={live:?} pending={pending:?} final={final_text:?} deltas={deltas:?} blocked={} stop_to_worker_done_ms={stop_to_worker_done_ms}",
            index + 1,
            clip.automatic,
            clip.expect_no_suffix,
            sink.borrow().blocked
        );
    }
    if worker::model_loads() != 1 {
        return Err("model was not reused".into());
    }
    Ok(())
}
