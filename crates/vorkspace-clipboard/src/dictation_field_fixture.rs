//! Debug-only end-to-end proof through Service, never an alternate transcriber
//! or paster. The source is pinned public PCM; the destination is a tagged,
//! disposable native field. No mic, hotkeys, socket, database or preferences.
use super::*;
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, ffi::CStr};
const PREFIX: &str = "Existing: ";
const JFK: &str = "And so, my fellow Americans, ask not what your country can do for you. Ask what you can do for your country";
const FLEURS_TDT: &str = "However, due to the slow communication channels, styles in the west could lag behind by 25-30 years.";
const FLEURS_EOU: &str = "however due to the slow communication channels styles in the west could lag behind by twenty five to thirty years";
unsafe extern "C" {
    fn vorkspace_dictation_fixture_read(pid: i32, receive: extern "C" fn(*const c_char)) -> i32;
    fn vorkspace_dictation_fixture_admit(pid: i32, epoch: u64) -> i32;
    fn vorkspace_dictation_fixture_reset(pid: i32) -> i32;
}
fn field(pid: i32) -> Result<String, String> {
    thread_local! { static TEXT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) }; }
    extern "C" fn receive(p: *const c_char) {
        TEXT.with(|v| {
            *v.borrow_mut() = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        });
    }
    native(unsafe { vorkspace_dictation_fixture_read(pid, receive) })?;
    Ok(TEXT.with(|v| std::mem::take(&mut *v.borrow_mut())))
}
fn native(code: i32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!(
            "Disposable field refused, native code {code}; no permission requested"
        ))
    }
}

/// Acknowledgements are actual AX values, not successful CGEvent post calls.
/// Every attempted delivery must be observed in order; unexpected edits fail
/// the run without rewriting the field. Coalescing happens only in Service.
struct FieldLedger {
    expected: &'static str,
    issued: u64,
    previous: String,
    visible: String,
    pending: VecDeque<(u64, String)>,
}
impl FieldLedger {
    fn new(expected: &'static str) -> Self {
        Self {
            expected,
            issued: 0,
            previous: String::new(),
            visible: PREFIX.into(),
            pending: VecDeque::new(),
        }
    }
    fn issue(&mut self, sequence: u64, committed: &str) -> Result<(), String> {
        if sequence != self.issued + 1
            || !committed.starts_with(&self.previous)
            || committed == self.previous
        {
            return Err("Invalid or repeated controller delivery receipt".into());
        }
        self.previous = committed.into();
        self.issued = sequence;
        self.pending
            .push_back((sequence, format!("{PREFIX}{committed}")));
        Ok(())
    }
    fn observe(&mut self, text: &str) -> Result<Option<u64>, String> {
        if text == self.visible {
            return Ok(None);
        }
        let Some((sequence, expected)) = self.pending.front() else {
            return Err("Unexpected field edit after delivery".into());
        };
        if text != expected {
            return Err("Actual field differs from the next committed delivery; stopping without correction".into());
        }
        let sequence = *sequence;
        self.visible = text.into();
        self.pending.pop_front();
        Ok(Some(sequence))
    }
    fn complete(&self) -> bool {
        self.pending.is_empty() && self.visible == format!("{PREFIX}{}", self.expected)
    }
}

pub fn run(model: PathBuf, wav: PathBuf, pid: i32, selected: ModelId) {
    // Refuse before reading audio/loading weights if normal AX trust or the
    // disposable target is absent. This is not a permission request.
    match field(pid) {
        Ok(text) if text == PREFIX => {}
        Ok(_) => {
            eprintln!("Disposable field must contain only the known fixture prefix");
            std::process::exit(9);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    }
    let (pcm, expected) = public_pcm(&wav, selected).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let exit = std::rc::Rc::new(std::cell::Cell::new(1));
    let result_code = exit.clone();
    gpui::Application::with_platform(std::rc::Rc::new(gpui_macos::MacPlatform::new(false)))
        .with_windowing(gpui::WindowingRequest::Headless)
        .run(move |cx| {
            cx.set_global(crate::Helper::default());
            let service = cx.new(|_| {
                let mut s = Service::new(Ok(model));
                s.public_audio_fixture = true;
                s.initializing = false;
                s.attempt.enabled = true;
                s.selected = selected;
                s
            });
            service.update(cx, |s, cx| s.prepare_worker(cx));
            let result_code = result_code.clone();
            cx.spawn(async move |cx| {
                let result = exercise(&service, &pcm, pid, expected, cx).await;
                // Same cancellation/retirement owner and actual join as normal
                // disable, followed by the normal clipboard read window.
                service.update(cx, |s, cx| { s.attempt.enabled = false; s.cancel(cx); s.retire_worker(); });
                while service.read_with(cx, |s, _| s.worker.running) {
                    cx.background_executor().timer(Duration::from_millis(20)).await;
                }
                cx.background_executor().timer(Duration::from_millis(850)).await;
                unsafe { vorkspace_dictation_shutdown() };
                match result {
                    Ok(()) => { println!("{}", serde_json::json!({"full_flow": "passed", "turns": 2, "loaded_models": worker::model_loads(), "worker_joined": true, "microphone_opened": false})); result_code.set(0); }
                    Err(e) => eprintln!("{}", serde_json::json!({"full_flow":"failed", "error":e, "microphone_opened":false})),
                }
                cx.update(|cx| cx.quit());
            }).detach();
        });
    std::process::exit(exit.get());
}
fn public_pcm(
    path: &std::path::Path,
    selected: ModelId,
) -> Result<(Vec<f32>, &'static str), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let (hash, expected) = match selected {
        ModelId::UnifiedEnglish => (
            "59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e",
            JFK,
        ),
        ModelId::ParakeetTdtV3 => (
            "7835bd6ffb54ce38a2a9bcde3905ba424faed94d50a474f21a9cbe9209b869df",
            FLEURS_TDT,
        ),
        ModelId::ParakeetRealtimeEou => (
            "7835bd6ffb54ce38a2a9bcde3905ba424faed94d50a474f21a9cbe9209b869df",
            FLEURS_EOU,
        ),
    };
    if format!("{:x}", Sha256::digest(&bytes)) != hash {
        return Err(
            "Only the pinned public WAV for this model is allowed in the field fixture".into(),
        );
    }
    let wav = hound::WavReader::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let spec = wav.spec();
    if spec.sample_rate != 16000 || spec.channels != 1 || spec.bits_per_sample != 16 {
        return Err("Invalid public PCM format".into());
    }
    let pcm = wav
        .into_samples::<i16>()
        .map(|s| s.map(|v| v as f32 / 32768.).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((pcm, expected))
}
async fn exercise(
    service: &Entity<Service>,
    pcm: &[f32],
    pid: i32,
    expected: &'static str,
    cx: &mut gpui::AsyncApp,
) -> Result<(), String> {
    let cold = Instant::now();
    while !service.read_with(cx, |s, _| s.ready) {
        if !service.read_with(cx, |s, _| s.worker.running)
            || cold.elapsed() > Duration::from_secs(60)
        {
            return Err("Worker did not prepare".into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    println!(
        "{}",
        serde_json::json!({"prepare_ms": cold.elapsed().as_millis(), "loaded_models": worker::model_loads(), "route":"Service -> worker_event -> deliver -> native paste -> actual AX value"})
    );
    for turn in 1..=2 {
        if field(pid)? != PREFIX {
            return Err("Disposable prefix missing before turn".into());
        }
        let epoch = service
            .update(cx, |s, _| s.attempt.begin())
            .ok_or("Turn admission refused")?;
        native(unsafe { vorkspace_dictation_fixture_admit(pid, epoch) })?;
        service.update(cx, |s, _| s.begin_turn(epoch))?;
        let arming = Instant::now();
        while service.read_with(cx, |s, _| s.attempt.phase) == Phase::Starting {
            if arming.elapsed() > Duration::from_secs(10) {
                return Err("Turn did not arm".into());
            }
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
        }
        if service.read_with(cx, |s, _| s.attempt.phase) != Phase::Recording {
            return Err("Turn failed while arming".into());
        }
        let began = Instant::now();
        let mut next_feed = began + Duration::from_millis(100);
        let mut next_control = began;
        let mut offset = 0;
        let mut stop = None;
        let mut first = None;
        let mut ledger = FieldLedger::new(expected);
        loop {
            let now = Instant::now();
            if now.duration_since(began) > Duration::from_secs(30) {
                return Err("Full-flow turn exceeded its bounded deadline".into());
            }
            if now >= next_feed && stop.is_none() {
                let end = (offset + 1600).min(pcm.len());
                service.read_with(cx, |s, _| s.feed_audio(&pcm[offset..end]))?;
                offset = end;
                next_feed += Duration::from_millis(100);
                if offset == pcm.len() {
                    stop = Some(Instant::now());
                    service.update(cx, |s, _| {
                        if s.attempt.stop() != Some(epoch) {
                            return Err("Finish admission refused".into());
                        }
                        s.finish_input(worker::FinishReason::UserStop)
                    })?;
                }
            }
            if now >= next_control {
                service.update(cx, |s, cx| s.deliver(epoch, cx));
                next_control += Duration::from_millis(100);
            }
            let (sequence, text, phase, active, status) = service.read_with(cx, |s, _| {
                (
                    s.delivery,
                    s.delivered.text().to_owned(),
                    s.attempt.phase,
                    s.turn.is_some(),
                    s.status.clone(),
                )
            });
            if sequence > ledger.issued {
                ledger.issue(sequence, &text)?;
            }
            if let Some(sequence) = ledger.observe(&field(pid)?)? {
                let observed = Instant::now();
                first.get_or_insert(observed);
                println!(
                    "{}",
                    serde_json::json!({"turn":turn,"delivery":sequence,"field_ack_ms":observed.duration_since(began).as_millis(),"before_finish":stop.is_none(),"actual_field":ledger.visible})
                );
            }
            if !active {
                if phase != Phase::Idle || stop.is_none() {
                    return Err(format!("Controller stopped unexpectedly: {status}"));
                }
                if ledger.complete() {
                    break;
                }
            }
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
        }
        let stop = stop.ok_or("Missing Finish receipt")?;
        let first = first.ok_or("No actual field acknowledgement")?;
        if first >= stop {
            return Err("Field gained no stable words before Finish".into());
        }
        let final_field = Instant::now();
        cx.background_executor()
            .timer(Duration::from_millis(850))
            .await;
        ledger.observe(&field(pid)?)?;
        if !ledger.complete() || worker::model_loads() != 1 {
            return Err("Final field or warm reuse changed".into());
        }
        println!(
            "{}",
            serde_json::json!({"turn":turn,"first_actual_field_ms":first.duration_since(began).as_millis(),"recording_ms":stop.duration_since(began).as_millis(),"stop_to_final_field_ms":final_field.saturating_duration_since(stop).as_millis(),"clipboard_read_window_ms":800,"deliveries":ledger.issued,"final_field":ledger.visible,"loaded_models":worker::model_loads(),"final_stayed_unchanged":true})
        );
        if turn == 1 {
            native(unsafe { vorkspace_dictation_fixture_reset(pid) })?;
            let reset = Instant::now();
            while field(pid)? != PREFIX {
                if reset.elapsed() > Duration::from_secs(2) {
                    return Err("Disposable target did not acknowledge its own reset".into());
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn field_acknowledgements_require_actual_ordered_unicode_contents() {
        let mut ledger = FieldLedger::new(JFK);
        ledger.issue(1, "Héllo ").unwrap();
        assert_eq!(ledger.observe(PREFIX).unwrap(), None);
        assert_eq!(ledger.observe("Existing: Héllo ").unwrap(), Some(1));
        ledger.issue(2, "Héllo 世界").unwrap();
        assert_eq!(ledger.observe("Existing: Héllo 世界").unwrap(), Some(2));
        assert!(ledger.observe("Existing: Héllo 世界世界").is_err());
    }
    #[test]
    fn missing_intermediate_ack_and_replayed_delivery_fail_the_proof() {
        let mut ledger = FieldLedger::new(JFK);
        ledger.issue(1, "one ").unwrap();
        ledger.issue(2, "one two").unwrap();
        assert!(ledger.observe("Existing: one two").is_err());
        assert!(ledger.issue(2, "one two").is_err());
        assert!(!ledger.complete());
    }
    #[test]
    fn full_final_ack_is_required_and_a_later_edit_is_not_corrected() {
        let mut ledger = FieldLedger::new(JFK);
        ledger.issue(1, JFK).unwrap();
        assert!(!ledger.complete());
        ledger.observe(&format!("{PREFIX}{JFK}")).unwrap();
        assert!(ledger.complete());
        assert!(ledger.observe("Existing: user edit").is_err());
    }
    #[test]
    fn tdt_field_proof_requires_the_tdt_fixture_transcript() {
        let mut ledger = FieldLedger::new(FLEURS_TDT);
        ledger.issue(1, FLEURS_TDT).unwrap();
        ledger.observe(&format!("{PREFIX}{FLEURS_TDT}")).unwrap();
        assert!(ledger.complete());
    }
    #[test]
    fn flash_field_proof_requires_the_full_public_transcript() {
        let mut ledger = FieldLedger::new(FLEURS_EOU);
        ledger.issue(1, "however due to ").unwrap();
        ledger.observe("Existing: however due to ").unwrap();
        assert!(!ledger.complete());
        ledger.issue(2, FLEURS_EOU).unwrap();
        ledger.observe(&format!("{PREFIX}{FLEURS_EOU}")).unwrap();
        assert!(ledger.complete());
    }
}
