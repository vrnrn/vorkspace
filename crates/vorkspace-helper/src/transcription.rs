//! One recorder owns the shared voice lease until native cleanup and durable Stop.
use gpui::{App, AppContext, Context, Entity, Global};
use serde::Deserialize;
use std::{
    ffi::{CStr, CString, c_char},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use vorkspace_helper::{AudioNotice, AudioRequest, Notice, word_rules};
use vorkspace_macos::voice_lease::Lease;
use vorkspace_meetings::{
    Meetings,
    recording::{self, CaptureSource, Owner, Phase, State},
};
unsafe extern "C" {
    fn vorkspace_transcription_start(json: *const c_char, callback: extern "C" fn(*const c_char));
    fn vorkspace_transcription_finish();
    fn vorkspace_transcription_cancel();
}
static EVENTS: OnceLock<Events> = OnceLock::new();
struct Events {
    sender: async_channel::Sender<Event>,
    terminal: Mutex<Option<Event>>,
}
impl Events {
    fn push(&self, event: Event) -> bool {
        match self.sender.try_send(event) {
            Ok(()) => true,
            Err(error) => {
                let event = error.into_inner();
                // A full text queue may stop capture, but it must never swallow
                // native cleanup's final receipt and strand the microphone lease.
                if event.phase.as_ref().is_some_and(|phase| !phase.active()) {
                    *self.terminal.lock().unwrap() = Some(event);
                }
                false
            }
        }
    }
    fn terminal_after_drain(&self) -> Option<Event> {
        if self.sender.is_empty() {
            self.terminal.lock().unwrap().take()
        } else {
            None
        }
    }
}
static OVERFLOW: AtomicBool = AtomicBool::new(false);
static ACTIVE_RUN: Mutex<Option<String>> = Mutex::new(None);
#[derive(Deserialize)]
struct Event {
    id: String,
    run: String,
    phase: Option<Phase>,
    source: Option<String>,
    text: Option<String>,
    #[serde(default, rename = "final")]
    finalized: bool,
    #[serde(default)]
    start_ms: u64,
    #[serde(default)]
    end_ms: u64,
    error: Option<String>,
}
extern "C" fn event(json: *const c_char) {
    let bytes = unsafe { CStr::from_ptr(json) }.to_bytes();
    if let Ok(event) = serde_json::from_slice::<Event>(bytes)
        && let Some(events) = EVENTS.get()
    {
        let active = ACTIVE_RUN.lock().unwrap();
        if active.as_deref() == Some(event.run.as_str()) && !events.push(event) {
            OVERFLOW.store(true, Ordering::Release);
            drop(active);
            unsafe { vorkspace_transcription_cancel() };
        }
    }
}
struct Audio(Entity<Service>);
impl Global for Audio {}
#[derive(Default)]
struct Service {
    recording: Option<Recording>,
}
struct Recording {
    request: AudioRequest,
    owner: Owner,
    _lease: Lease,
    error: Option<String>,
}
impl Recording {
    fn status(&self, ended: bool) -> State {
        let mut state = self.owner.state.clone();
        // A failed disk commit cannot be advertised as a successful Stop. Keep
        // the live receipt honest even when durable recovery can only report interruption.
        if let Some(error) = &self.error {
            state.error = Some(error.clone());
            state.phase = if ended {
                Phase::Failed
            } else {
                Phase::Finishing
            };
            if ended {
                state.ended_at = Some(recording::now_ms());
            }
        }
        state
    }
}
fn get(cx: &App) -> Entity<Service> {
    cx.global::<Audio>().0.clone()
}
pub fn init(cx: &mut App) {
    let service = cx.new(|_| Service::default());
    cx.set_global(Audio(service.clone()));
    let (tx, rx) = async_channel::bounded(64);
    let _ = EVENTS.set(Events {
        sender: tx,
        terminal: Mutex::new(None),
    });
    service.update(cx, |_, cx| {
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let _ = this.update(cx, |s, cx| s.event(event, cx));
                if let Some(terminal) = EVENTS.get().and_then(Events::terminal_after_drain) {
                    let _ = this.update(cx, |s, cx| s.event(terminal, cx));
                }
            }
        })
        .detach();
    });
}
pub fn active(cx: &App) -> bool {
    cx.try_global::<Audio>()
        .is_some_and(|a| a.0.read(cx).recording.is_some())
}
#[cfg(not(test))]
pub(crate) fn active_run(cx: &App) -> Option<&str> {
    Some(
        cx.try_global::<Audio>()?
            .0
            .read(cx)
            .recording
            .as_ref()?
            .request
            .run
            .as_str(),
    )
}
pub fn start(request: AudioRequest, cx: &mut App) {
    get(cx).update(cx, |s, cx| s.start(request, cx));
}
pub fn finish(cx: &mut App) {
    if active(cx) {
        unsafe { vorkspace_transcription_finish() }
    }
}
pub fn finish_owned(id: &str, owner: &str, run: &str, cx: &mut App) {
    let matches =
        get(cx).read(cx).recording.as_ref().is_some_and(|r| {
            r.request.id == id && r.request.owner == owner && r.request.run == run
        });
    if matches {
        finish(cx)
    }
}
pub fn status(cx: &App) {
    if let Some(recording) = &get(cx).read(cx).recording {
        crate::notify(
            &Notice::Audio(AudioNotice::State {
                owner: recording.request.owner.clone(),
                state: recording.status(false),
            }),
            cx,
        );
    } else {
        crate::notify(&Notice::Audio(AudioNotice::Idle), cx)
    }
}
impl Service {
    fn state(&self, ended: bool, cx: &mut App) {
        if let Some(r) = &self.recording {
            let state = r.status(ended);
            crate::meeting_panel::update(
                &state,
                |id, run, cx| finish_owned(id, "meetings", run, cx),
                cx,
            );
            crate::notify(
                &Notice::Audio(AudioNotice::State {
                    owner: r.request.owner.clone(),
                    state,
                }),
                cx,
            )
        }
    }
    fn start(&mut self, request: AudioRequest, cx: &mut Context<Self>) {
        if let Some(recording) = &self.recording {
            let state = if recording.request.id == request.id
                && recording.request.owner == request.owner
                && recording.request.run == request.run
            {
                recording.status(false)
            } else {
                failed_start(
                    &request,
                    "Another recording is still stopping. Try again after it finishes.".into(),
                )
            };
            crate::notify(
                &Notice::Audio(AudioNotice::State {
                    owner: request.owner,
                    state,
                }),
                cx,
            );
            return;
        }
        let result = (|| {
            if !vorkspace_meetings::valid_id(&request.id)
                || !vorkspace_meetings::valid_id(&request.owner)
                || !vorkspace_meetings::valid_id(&request.run)
            {
                return Err("Invalid recording identity".into());
            }
            let rules = word_rules(&request.words)?;
            let lease = Lease::acquire()?;
            let owner = Meetings {
                directory: request.data_dir.clone(),
            }
            .start_recording(
                &request.id,
                &request.run,
                request.previous_run.as_deref(),
                request.source,
            )?;
            Ok((lease, owner, rules))
        })();
        let (lease, owner, rules) = match result {
            Ok(v) => v,
            Err(error) => {
                let state = recording::state(&request.data_dir, &request.id)
                    .ok()
                    .filter(|state| state.run.as_deref() == Some(request.run.as_str()))
                    .unwrap_or_else(|| failed_start(&request, error));
                crate::notify(
                    &Notice::Audio(AudioNotice::State {
                        owner: request.owner,
                        state,
                    }),
                    cx,
                );
                return;
            }
        };
        let json = CString::new(
            serde_json::json!({
                "id": request.id,
                "run": request.run,
                "captureSystem": request.source == CaptureSource::MicrophoneAndSystem,
                "microphone": request.microphone,
                "rules": rules,
            })
            .to_string(),
        )
        .unwrap();
        OVERFLOW.store(false, Ordering::Release);
        *ACTIVE_RUN.lock().unwrap() = Some(request.run.clone());
        self.recording = Some(Recording {
            request,
            owner,
            _lease: lease,
            error: None,
        });
        self.state(false, cx);
        crate::service::meeting_recording_changed(
            self.recording.as_ref().map(|r| r.request.run.as_str()),
            cx,
        );
        crate::dictation::audio_changed(true, cx);
        unsafe { vorkspace_transcription_start(json.as_ptr(), event) };
    }
    fn event(&mut self, event: Event, cx: &mut Context<Self>) {
        let Some(r) = &mut self.recording else { return };
        if r.request.id != event.id || r.request.run != event.run {
            return;
        }
        if OVERFLOW.swap(false, Ordering::AcqRel) {
            r.error =
                Some("Speech updates exceeded their queue limit. Saved text is preserved.".into());
        }
        if let Some(phase) = event.phase {
            let ended = !phase.active();
            let phase = if ended && r.error.is_some() {
                Phase::Failed
            } else {
                phase
            };
            if let Err(error) = r
                .owner
                .phase(phase, event.error.or_else(|| r.error.clone()))
            {
                r.error = Some(format!("Transcript storage failed: {error}"));
                unsafe { vorkspace_transcription_cancel() };
            }
            self.state(ended, cx);
            if ended {
                *ACTIVE_RUN.lock().unwrap() = None;
                self.recording = None;
                crate::meeting_panel::close(cx);
                crate::service::meeting_recording_changed(None, cx);
                cx.spawn(async move |_, cx| {
                    cx.update(|cx| {
                        // A new take can be admitted before this deferred cleanup.
                        if !active(cx) {
                            crate::dictation::audio_changed(false, cx);
                            crate::audio_finished(cx);
                        }
                    });
                })
                .detach();
            }
            return;
        }
        if r.error.is_some() {
            return;
        }
        let (Some(source), Some(text)) = (event.source, event.text) else {
            return;
        };
        if !r.request.source.allows(&source) {
            r.error = Some("Unknown audio source".into());
            unsafe { vorkspace_transcription_cancel() };
            return;
        }
        let notice = if event.finalized && !text.trim().is_empty() {
            match r.owner.append(&source, event.start_ms, event.end_ms, text) {
                Ok(segment) => AudioNotice::Segment {
                    id: r.request.id.clone(),
                    owner: r.request.owner.clone(),
                    run: r.request.run.clone(),
                    segment,
                },
                Err(error) => {
                    r.error = Some(error);
                    unsafe { vorkspace_transcription_cancel() };
                    return;
                }
            }
        } else {
            AudioNotice::Partial {
                id: r.request.id.clone(),
                owner: r.request.owner.clone(),
                run: r.request.run.clone(),
                source,
                text,
            }
        };
        crate::notify(&Notice::Audio(notice), cx);
    }
}

fn failed_start(request: &AudioRequest, error: String) -> State {
    let mut state = State::starting(request.id.clone(), request.run.clone(), request.source);
    state.phase = Phase::Failed;
    state.ended_at = Some(recording::now_ms());
    state.error = Some(error);
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(phase: Option<Phase>) -> Event {
        Event {
            id: "sample".into(),
            run: "sample-run".into(),
            phase,
            source: None,
            text: None,
            finalized: false,
            start_ms: 0,
            end_ms: 0,
            error: None,
        }
    }
    #[test]
    fn queue_overflow_retains_cleanup_after_all_preceding_text() {
        let (sender, receiver) = async_channel::bounded(1);
        let events = Events {
            sender,
            terminal: Mutex::new(None),
        };
        assert!(events.push(event(None)));
        assert!(!events.push(event(Some(Phase::Failed))));
        assert!(events.terminal_after_drain().is_none());
        assert!(receiver.try_recv().unwrap().phase.is_none());
        assert_eq!(
            events.terminal_after_drain().unwrap().phase,
            Some(Phase::Failed)
        );
        assert!(events.terminal_after_drain().is_none());
    }
}
