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
use vorkspace_plugin::recording::{Owner, Phase};
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
#[derive(Deserialize)]
struct Event {
    id: String,
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
    if let Ok(event) = serde_json::from_slice(bytes)
        && let Some(events) = EVENTS.get()
        && !events.push(event)
    {
        OVERFLOW.store(true, Ordering::Release);
        unsafe { vorkspace_transcription_cancel() };
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
    fn status(&self, ended: bool) -> vorkspace_plugin::recording::State {
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
                state.ended_at = Some(vorkspace_plugin::recording::now_ms());
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
pub fn start(request: AudioRequest, cx: &mut App) {
    get(cx).update(cx, |s, cx| s.start(request, cx));
}
pub fn finish(cx: &mut App) {
    if active(cx) {
        unsafe { vorkspace_transcription_finish() }
    }
}
pub fn finish_owned(id: &str, owner: &str, cx: &mut App) {
    let matches = get(cx)
        .read(cx)
        .recording
        .as_ref()
        .is_some_and(|r| r.request.id == id && r.request.owner == owner);
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
    fn state(&self, ended: bool, cx: &App) {
        if let Some(r) = &self.recording {
            crate::notify(
                &Notice::Audio(AudioNotice::State {
                    owner: r.request.owner.clone(),
                    state: r.status(ended),
                }),
                cx,
            )
        }
    }
    fn start(&mut self, request: AudioRequest, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            let state = vorkspace_plugin::recording::State {
                id: request.id,
                started_at: vorkspace_plugin::recording::now_ms(),
                ended_at: Some(vorkspace_plugin::recording::now_ms()),
                phase: Phase::Failed,
                error: Some(
                    "Another recording is still stopping. Try again after it finishes.".into(),
                ),
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
            if !vorkspace_plugin::valid_id(&request.id)
                || !vorkspace_plugin::valid_id(&request.owner)
            {
                return Err("Invalid recording identity".into());
            }
            let rules = word_rules(&request.words)?;
            let lease = Lease::acquire()?;
            let owner = Owner::create(&request.data_dir, &request.id)?;
            Ok((lease, owner, rules))
        })();
        let (lease, owner, rules) = match result {
            Ok(v) => v,
            Err(error) => {
                let state = vorkspace_plugin::recording::State {
                    id: request.id,
                    started_at: vorkspace_plugin::recording::now_ms(),
                    ended_at: Some(vorkspace_plugin::recording::now_ms()),
                    phase: Phase::Failed,
                    error: Some(error),
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
        };
        let json = CString::new(
            serde_json::json!({"id":request.id,"microphone":request.microphone,"rules":rules})
                .to_string(),
        )
        .unwrap();
        self.recording = Some(Recording {
            request,
            owner,
            _lease: lease,
            error: None,
        });
        OVERFLOW.store(false, Ordering::Release);
        self.state(false, cx);
        crate::dictation::audio_changed(true, cx);
        unsafe { vorkspace_transcription_start(json.as_ptr(), event) };
    }
    fn event(&mut self, event: Event, cx: &mut Context<Self>) {
        let Some(r) = &mut self.recording else { return };
        if r.request.id != event.id {
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
                self.recording = None;
                cx.spawn(async move |_, cx| {
                    cx.update(|cx| {
                        crate::dictation::audio_changed(false, cx);
                        crate::audio_finished(cx);
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
        if !["microphone", "system"].contains(&source.as_str()) {
            r.error = Some("Unknown audio source".into());
            unsafe { vorkspace_transcription_cancel() };
            return;
        }
        let notice = if event.finalized && !text.trim().is_empty() {
            match r.owner.append(&source, event.start_ms, event.end_ms, text) {
                Ok(segment) => AudioNotice::Segment {
                    id: r.request.id.clone(),
                    owner: r.request.owner.clone(),
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
                source,
                text,
            }
        };
        crate::notify(&Notice::Audio(notice), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(phase: Option<Phase>) -> Event {
        Event {
            id: "sample".into(),
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
