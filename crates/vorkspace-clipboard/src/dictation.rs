//! Dictation lives in the clipboard process. IPC carries configuration and
//! status, never PCM or recognized transcript text.
use crate::dictation_panel as panel;
#[cfg(debug_assertions)]
#[path = "dictation_engine_fixture.rs"]
pub(crate) mod engine_fixture;
#[cfg(debug_assertions)]
#[path = "dictation_field_fixture.rs"]
pub(crate) mod field_fixture;
use gpui::{App, AppContext, Context, Entity, Global, Task};
use std::sync::mpsc;
use std::{
    ffi::{CString, c_char},
    path::PathBuf,
    time::{Duration, Instant},
};
use transcribe_cpp::CancelToken;
use vorkspace_clipboard::dictation_engine_lease::{AcquireError, EngineLease};
use vorkspace_clipboard::dictation_worker::{self as worker, CommittedPrefix, Event, Pipe, Turn};
use vorkspace_clipboard::{ModelId, ModelState, dictation_model as model, dictation_path};
use vorkspace_clipboard::{
    Notice,
    dictation_state::{Attempt, CaptureHealth, DownloadSlot, Phase},
};
use vorkspace_macos::{global_hotkey::Registration, voice_lease::Lease};
unsafe extern "C" {
    fn vorkspace_dictation_shutdown();
    fn vorkspace_dictation_admit(epoch: u64) -> i32;
    fn vorkspace_dictation_start(microphone: *const c_char, epoch: u64) -> i32;
    fn vorkspace_dictation_drain(callback: extern "C" fn(*const f32, usize));
    fn vorkspace_dictation_target_current() -> bool;
    fn vorkspace_dictation_stop(callback: extern "C" fn(*const f32, usize)) -> i32;
    fn vorkspace_dictation_cancel();
    fn vorkspace_dictation_health() -> i32;
    fn vorkspace_dictation_permissions();
    fn vorkspace_dictation_paste(text: *const c_char, epoch: u64, delivery: u64) -> i32;
    fn vorkspace_dictation_modifiers_down() -> bool;
}
struct Dictation(Entity<Service>);
impl Global for Dictation {}
#[derive(Default)]
struct WorkerSlot {
    running: bool,
    generation: u64,
}
impl WorkerSlot {
    fn begin(&mut self) -> Option<u64> {
        if self.running {
            return None;
        }
        self.running = true;
        self.generation += 1;
        Some(self.generation)
    }
    /// Called after the worker has joined, or when no worker could be started.
    fn joined(&mut self, generation: u64) -> bool {
        if !self.running || self.generation != generation {
            return false;
        }
        self.running = false;
        true
    }
}
pub struct Service {
    #[cfg(debug_assertions)]
    public_audio_fixture: bool,
    #[cfg(debug_assertions)]
    fixture_engine_lock: Option<PathBuf>,
    warm: Option<mpsc::SyncSender<Turn>>,
    worker: WorkerSlot,
    retry_epoch: u64,
    retry_pending: bool,
    worker_cancel: CancelToken,
    turn: Option<(u64, Pipe)>,
    native_started: bool,
    pending_text: String,
    delivered: CommittedPrefix,
    delivery: u64,
    finished: Option<Result<(), String>>,
    modifier_deadline: Option<Instant>,
    attempt: Attempt,
    lease: Option<Lease>,
    hotkey: Option<(Registration, Task<()>)>,
    shortcut_error: Option<String>,
    cancel: CancelToken,
    download_cancel: CancelToken,
    downloads: DownloadSlot,
    pending_download: Option<(u64, ModelId)>,
    selected: ModelId,
    microphone: String,
    words: Result<Vec<vorkspace_core::dictation_words::Rule>, String>,
    capture_model: Option<ModelId>,
    capture_microphone: Option<String>,
    download_model: Option<ModelId>,
    model_notice: Option<(u64, ModelId, ModelState)>,
    initializing: bool,
    verification: u64,
    ready: bool,
    status: String,
    model: Result<PathBuf, String>,
    panel: Option<gpui::WindowHandle<panel::Panel>>,
}
pub fn init(cx: &mut App) {
    let path = dictation_path::model_path(ModelId::default());
    let service = cx.new(|_| Service::new(path));
    cx.set_global(Dictation(service.clone()));
    service.update(cx, |s, cx| s.check_ready(cx));
}

fn get(cx: &App) -> Entity<Service> {
    cx.global::<Dictation>().0.clone()
}
pub fn status(cx: &App) {
    get(cx).read(cx).publish(cx)
}
pub fn configure(enabled: bool, selected: ModelId, microphone: &str, words: &str, cx: &mut App) {
    get(cx).update(cx, |s, cx| {
        // A current attempt owns its device until stop/cancel; preference changes
        // affect the next admission, including after the main app quits.
        s.microphone = microphone.into();
        // Each admitted turn takes its own immutable snapshot. A malformed
        // configuration fails the next admission rather than using stale rules.
        s.words = vorkspace_core::dictation_words::parse(words);
        let changed = s.selected != selected;
        if s.attempt.enabled == enabled && !changed {
            return;
        }
        s.retry_epoch = s.retry_epoch.wrapping_add(1);
        s.retry_pending = false;
        // Retire the previous owner before changing model identity. Cancelling
        // inference/download keeps its worker slot until the completion joins.
        if changed || !enabled {
            s.hotkey = None;
            s.cancel(cx);
            s.retire_worker();
            if let Some((request, model)) = s
                .pending_download
                .or_else(|| s.downloads.id().zip(s.download_model))
            {
                s.cancel_download(request, model, cx);
            }
        }
        s.attempt.enabled = enabled;
        if changed {
            s.selected = selected;
            s.model = dictation_path::model_path(selected);
            s.verification += 1;
            s.ready = false;
        }
        if !enabled {
            if changed {
                s.check_ready(cx);
            }
            return;
        }
        s.check_ready(cx);
        s.shortcut_error = None;
        match Registration::new("cmd-,") {
            Ok((r, rx)) => {
                let task = cx.spawn(async move |this, cx| {
                    while rx.recv().await.is_ok() {
                        let _ = this.update(cx, |s, cx| s.toggle(cx));
                    }
                });
                s.hotkey = Some((r, task));
            }
            Err(e) => s.shortcut_error = Some(e),
        }
        s.publish(cx);
    });
}
pub fn permissions(_: &mut App) {
    unsafe { vorkspace_dictation_permissions() }
}
pub fn cancel(cx: &mut App) {
    get(cx).update(cx, |s, cx| {
        s.cancel(cx);
        if let Some((id, model)) = s
            .pending_download
            .or_else(|| s.downloads.id().zip(s.download_model))
        {
            s.cancel_download(id, model, cx);
        }
    })
}
pub fn cancel_model(request: u64, model: ModelId, cx: &mut App) {
    get(cx).update(cx, |s, cx| s.cancel_download(request, model, cx));
}
pub fn model_status(request: u64, model: ModelId, cx: &mut App) {
    get(cx).update(cx, |s, cx| {
        if let Some((id, saved_model, state)) = &s.model_notice
            && *id == request
            && *saved_model == model
        {
            crate::notify(
                &Notice::Model {
                    model,
                    request,
                    state: state.clone(),
                },
                cx,
            );
            return;
        }
        // Read-only recovery after reconnect or helper replacement; never replay download.
        let path = dictation_path::model_path(model);
        cx.spawn(async move |this, cx| {
            let ready = cx
                .background_executor()
                .spawn(
                    async move { path.and_then(|p| model::verify(model, &p, &CancelToken::new())) },
                )
                .await;
            let _ = this.update(cx, |_, cx| {
                crate::notify(
                    &Notice::Model {
                        model,
                        request,
                        state: if ready.is_ok() {
                            ModelState::Ready
                        } else {
                            ModelState::Missing
                        },
                    },
                    cx,
                )
            });
        })
        .detach();
    });
}
pub fn download(request: u64, model: ModelId, cx: &mut App) {
    get(cx).update(cx, |s, cx| {
        if model != s.selected {
            crate::notify(
                &Notice::Model {
                    request,
                    model,
                    state: ModelState::Cancelled,
                },
                cx,
            );
            return;
        }
        if s.initializing {
            if s.pending_download
                .is_some_and(|old| old != (request, model))
            {
                crate::notify(
                    &Notice::Model {
                        model,
                        request,
                        state: ModelState::Error(
                            "A model request is already checking. Cancel or wait, then retry."
                                .into(),
                        ),
                    },
                    cx,
                );
                return;
            }
            s.pending_download = Some((request, model));
            s.model_event(request, model, ModelState::Checking, cx);
        } else {
            s.start_download(request, model, cx);
        }
    });
}
impl Service {
    fn new(model: Result<PathBuf, String>) -> Self {
        Self {
            #[cfg(debug_assertions)]
            public_audio_fixture: false,
            #[cfg(debug_assertions)]
            fixture_engine_lock: None,
            warm: None,
            worker: WorkerSlot::default(),
            retry_epoch: 0,
            retry_pending: false,
            worker_cancel: CancelToken::new(),
            turn: None,
            native_started: false,
            pending_text: String::new(),
            delivered: CommittedPrefix::default(),
            delivery: 0,
            finished: None,
            modifier_deadline: None,
            attempt: Attempt::default(),
            lease: None,
            hotkey: None,
            shortcut_error: None,
            cancel: CancelToken::new(),
            download_cancel: CancelToken::new(),
            downloads: DownloadSlot::default(),
            pending_download: None,
            selected: ModelId::default(),
            microphone: "builtin".into(),
            words: Ok(Vec::new()),
            capture_model: None,
            capture_microphone: None,
            download_model: None,
            model_notice: None,
            initializing: true,
            verification: 0,
            ready: false,
            status: "Checking selected model…".into(),
            model,
            panel: None,
        }
    }

    fn check_ready(&mut self, cx: &mut Context<Self>) {
        self.verification += 1;
        let epoch = self.verification;
        let path = self.model.clone();
        let selected = self.selected;
        self.initializing = true;
        self.ready = false;
        self.status = format!("Checking {}…", selected.descriptor().label);
        self.publish(cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    path.as_ref()
                        .map(|p| model::verified(selected, p))
                        .map_err(Clone::clone)
                })
                .await;
            let _ = this.update(cx, |s, cx| s.checked(epoch, result, cx));
        })
        .detach();
    }
    fn checked(&mut self, epoch: u64, result: Result<bool, String>, cx: &mut Context<Self>) {
        if self.verification != epoch {
            return;
        }
        self.initializing = false;
        self.ready = result.as_ref() == Ok(&true);
        self.status = result.map_or_else(
            |e| e,
            |r| {
                if r && self.attempt.enabled {
                    "Ready · ⌘, to start and stop".into()
                } else if r {
                    format!(
                        "{} verified. Enable dictation to start.",
                        self.selected.descriptor().label
                    )
                } else {
                    format!("Download {} to begin.", self.selected.descriptor().label)
                }
            },
        );
        if self.ready && self.attempt.enabled {
            self.prepare_worker(cx);
        }
        self.publish(cx);
        if let Some((request, model)) = self.pending_download.take() {
            self.start_download(request, model, cx);
        }
    }

    fn model_event(&mut self, request: u64, model: ModelId, state: ModelState, cx: &App) {
        self.model_notice = Some((request, model, state.clone()));
        crate::notify(
            &Notice::Model {
                request,
                model,
                state,
            },
            cx,
        );
    }
    fn cancel_download(&mut self, request: u64, model: ModelId, cx: &mut Context<Self>) {
        if self.pending_download == Some((request, model)) {
            self.pending_download = None;
            self.model_event(request, model, ModelState::Cancelled, cx);
        } else if self.download_model == Some(model) && self.downloads.cancel(request) {
            self.download_cancel.cancel();
            self.model_event(request, model, ModelState::Cancelling, cx);
        } else {
            crate::notify(
                &Notice::Model {
                    model,
                    request,
                    state: ModelState::Cancelled,
                },
                cx,
            );
        }
    }
    fn start_download(&mut self, request: u64, model: ModelId, cx: &mut Context<Self>) {
        if self.pending_download == Some((request, model)) {
            self.pending_download = None;
        }
        if model != self.selected {
            self.model_event(request, model, ModelState::Cancelled, cx);
            return;
        }
        if self.downloads.id() == Some(request) && self.download_model == Some(model) {
            if let Some((id, saved_model, state)) = &self.model_notice
                && *id == request
                && *saved_model == model
            {
                crate::notify(
                    &Notice::Model {
                        model,
                        request,
                        state: state.clone(),
                    },
                    cx,
                );
            }
            return;
        }
        if self.attempt.phase != Phase::Idle {
            self.model_event(
                request,
                model,
                ModelState::Error(
                    "A dictation turn is still active. Finish or cancel it before downloading."
                        .into(),
                ),
                cx,
            );
            return;
        }
        if self.worker.running {
            self.retire_worker();
            self.pending_download = Some((request, model));
            self.model_event(request, model, ModelState::Checking, cx);
            return;
        }
        let path = match self.model.clone() {
            Ok(p) => p,
            Err(e) => {
                self.model_event(request, model, ModelState::Error(e), cx);
                return;
            }
        };
        if self.attempt.phase != Phase::Idle || !self.downloads.begin(request) {
            crate::notify(
                &Notice::Model {
                    model,
                    request,
                    state: ModelState::Error(
                        "A previous dictation or model job is still finishing. Retry in a moment."
                            .into(),
                    ),
                },
                cx,
            );
            return;
        }
        self.verification += 1;
        self.initializing = false;
        self.download_model = Some(model);
        self.download_cancel = CancelToken::new();
        let cancel = self.download_cancel.clone();
        self.model_event(request, model, ModelState::Downloading(0), cx);
        let (tx, rx) = async_channel::bounded(1);
        cx.spawn(async move |this, cx| {
            while let Ok(bytes) = rx.recv().await {
                let _ = this.update(cx, |s, cx| {
                    if s.selected == model
                        && s.download_model == Some(model)
                        && s.downloads.accepts(request)
                    {
                        s.model_event(request, model, ModelState::Downloading(bytes), cx);
                    }
                });
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    model::download(model, &path, &cancel, |bytes| {
                        let _ = tx.try_send(bytes);
                    })
                })
                .await;
            let _ = this.update(cx, |s, cx| {
                let Some(publish) = s.downloads.finish(request) else {
                    return;
                };
                s.download_model = None;
                let state = if !publish || s.selected != model {
                    ModelState::Cancelled
                } else {
                    match result {
                        Ok(()) => {
                            s.verification += 1;
                            s.initializing = false;
                            s.ready = true;
                            s.status = format!("{} verified.", model.descriptor().label);
                            ModelState::Ready
                        }
                        Err(e) => ModelState::Error(e),
                    }
                };
                s.model_event(request, model, state, cx);
                if s.attempt.enabled && s.selected == model {
                    s.check_ready(cx);
                } else {
                    s.publish(cx);
                }
                if let Some((pending, pending_model)) = s.pending_download.take() {
                    s.start_download(pending, pending_model, cx);
                }
            });
        })
        .detach();
    }

    fn publish(&self, cx: &App) {
        crate::notify(
            &Notice::Dictation {
                model: self.selected,
                status: self.shortcut_error.as_ref().map_or_else(
                    || self.status.clone(),
                    |e| format!("{}\n⌘,: {e}", self.status),
                ),
                ready: self.ready,
            },
            cx,
        );
        if let Some((request, model, state)) = &self.model_notice {
            crate::notify(
                &Notice::Model {
                    model: *model,
                    request: *request,
                    state: state.clone(),
                },
                cx,
            );
        }
    }
    fn display(&mut self, status: &str, state: panel::State, cx: &mut Context<Self>) -> bool {
        #[cfg(debug_assertions)]
        if self.public_audio_fixture {
            return true;
        }
        if let Some(p) = self.panel {
            if p.update(cx, |p, _, cx| {
                p.status = status.into();
                p.state = state;
                cx.notify();
            })
            .is_ok()
            {
                return true;
            }
            self.panel = None;
        }
        self.panel = panel::open(status, state, cx).ok();
        self.panel.is_some()
    }
    fn hide(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.panel.take() {
            let _ = p.update(cx, |p, w, cx| {
                p.placement.close();
                w.remove_window();
                cx.notify();
            });
        }
    }
    fn error(&mut self, message: String, cx: &mut Context<Self>) {
        self.status = message;
        self.publish(cx);
        self.display(&self.status.clone(), panel::State::Error, cx);
        let epoch = self.attempt.epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            let _ = this.update(cx, |s, cx| {
                if s.attempt.epoch == epoch && s.attempt.phase == Phase::Idle {
                    s.hide(cx);
                    s.lease = None;
                }
            });
        })
        .detach();
    }
    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.cancel.cancel();
        self.attempt.cancel();
        if self.turn.is_some() {
            if self.finished.is_some() {
                self.turn = None;
                self.attempt.phase = Phase::Idle;
            } else {
                self.attempt.phase = Phase::Cancelling;
            }
        }
        self.pending_text.clear();
        self.finished = None;
        self.native_started = false;
        self.capture_model = None;
        self.capture_microphone = None;
        unsafe { vorkspace_dictation_cancel() };
        self.hide(cx);
        if self.attempt.phase == Phase::Idle {
            self.lease = None;
        }
        self.status = "Cancelled · ⌘, to start again".into();
        self.publish(cx);
    }
    fn toggle(&mut self, cx: &mut Context<Self>) {
        if !self.attempt.enabled {
            return;
        }
        match self.attempt.phase {
            Phase::Starting => self.cancel(cx),
            Phase::Recording => self.stop(cx),
            Phase::Transcribing => self.cancel(cx),
            Phase::Cancelling => {}
            Phase::Idle => {
                self.hide(cx);
                self.lease = None;
                let lease = match Lease::acquire() {
                    Ok(l) => l,
                    Err(e) => {
                        self.status = e;
                        self.publish(cx);
                        return;
                    }
                };
                self.lease = Some(lease);
                if self.downloads.id().is_some() {
                    self.status = "Wait for the model download to finish.".into();
                    self.publish(cx);
                    self.lease = None;
                    return;
                }
                if self.initializing || (self.worker.running && !self.ready) {
                    self.status =
                        "Preparing selected model locally… Try dictation when Ready.".into();
                    self.publish(cx);
                    self.lease = None;
                    return;
                }
                if !self.ready {
                    self.attempt.epoch += 1;
                    self.error(
                        format!(
                            "Download {} in Settings → General → Dictation.",
                            self.selected.descriptor().label
                        ),
                        cx,
                    );
                    return;
                }
                let Some(epoch) = self.attempt.begin() else {
                    return;
                };
                self.capture_model = Some(self.selected);
                self.capture_microphone = Some(self.microphone.clone());
                let admitted = unsafe { vorkspace_dictation_admit(epoch) };
                if admitted != 0 {
                    self.attempt.cancel();
                    self.error(native_error(admitted).into(), cx);
                    return;
                }
                if let Err(e) = self.begin_turn(epoch) {
                    self.cancel(cx);
                    self.error(e, cx);
                    return;
                }
                if !self.display("Preparing dictation…", panel::State::Starting, cx) {
                    self.fail_turn(
                        "Could not show the dictation indicator. Try again.".into(),
                        cx,
                    );
                    return;
                }
                cx.spawn(async move |this, cx| {
                    let started = Instant::now();
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(100))
                            .await;
                        let keep = this
                            .update(cx, |s, cx| {
                                if s.turn.as_ref().map(|t| t.0) != Some(epoch) {
                                    return false;
                                }
                                if s.attempt.phase == Phase::Cancelling {
                                    return true;
                                }
                                if s.native_started
                                    && matches!(s.attempt.phase, Phase::Starting | Phase::Recording)
                                {
                                    if !unsafe { vorkspace_dictation_target_current() } {
                                        s.cancel(cx);
                                        return true;
                                    }
                                    let health = CaptureHealth::from(unsafe {
                                        vorkspace_dictation_health()
                                    });
                                    if !s.capture_health(epoch, health, cx) {
                                        return s.turn.is_some();
                                    }
                                    if let Err(e) = s.drain(false) {
                                        s.fail_turn(e, cx);
                                        return true;
                                    }
                                    if started.elapsed() >= Duration::from_secs(120) {
                                        s.stop(cx);
                                    }
                                }
                                s.deliver(epoch, cx);
                                s.turn.is_some()
                            })
                            .unwrap_or(false);
                        if !keep {
                            break;
                        }
                    }
                })
                .detach();
            }
        }
    }
    // Audio-source-independent production admission. The debug field fixture
    // substitutes public PCM for capture, then uses these same queues/events.
    fn begin_turn(&mut self, epoch: u64) -> Result<(), String> {
        if self.attempt.epoch != epoch || self.attempt.phase != Phase::Starting {
            return Err("Dictation turn was superseded before admission".into());
        }
        self.cancel = CancelToken::new();
        self.pending_text.clear();
        self.delivered = CommittedPrefix::default();
        self.delivery = 0;
        self.finished = None;
        self.modifier_deadline = None;
        self.native_started = false;
        let (pipe, input) = Pipe::new();
        let turn = Turn {
            epoch,
            input,
            cancel: self.cancel.clone(),
            words: self.words.clone()?,
        };
        if self.warm.as_ref().is_none_or(|w| w.try_send(turn).is_err()) {
            return Err("The local model is still preparing. Try again when Ready.".into());
        }
        self.turn = Some((epoch, pipe));
        Ok(())
    }
    fn feed_audio(&self, pcm: &[f32]) -> Result<(), String> {
        let pipe = &self
            .turn
            .as_ref()
            .ok_or("Missing dictation worker".to_string())?
            .1;
        for block in pcm.chunks(worker::SAMPLE_RATE) {
            pipe.audio(block.to_vec())?;
        }
        Ok(())
    }
    fn finish_input(&mut self) -> Result<(), String> {
        if self.attempt.phase != Phase::Transcribing {
            return Err("Finish was not admitted".into());
        }
        self.turn
            .as_ref()
            .ok_or("Missing dictation worker".to_string())?
            .1
            .finish()?;
        self.native_started = false;
        Ok(())
    }
    // Publication is derived from the existing attempt phase, in the same main
    // loop update that observes native readiness. Red never precedes acceptance.
    fn display_capture(&mut self, cx: &mut Context<Self>) -> bool {
        let (status, label, state) = match self.attempt.phase {
            Phase::Starting => (
                "Starting microphone… · ⌘, to cancel",
                "Starting microphone…",
                panel::State::Starting,
            ),
            Phase::Recording => (
                "Recording · ⌘, to finish",
                "Recording",
                panel::State::Recording,
            ),
            _ => return false,
        };
        self.status = status.into();
        self.publish(cx);
        if self.display(label, state, cx) {
            return true;
        }
        self.cancel(cx);
        self.status =
            "Could not show the recording indicator. Dictation cancelled; try again.".into();
        self.publish(cx);
        false
    }
    // Shared production control boundary for the poll and deterministic tests.
    // A cancelled/superseded attempt cannot publish readiness or admit inference.
    fn capture_health(
        &mut self,
        epoch: u64,
        health: CaptureHealth,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.attempt.epoch != epoch
            || !matches!(self.attempt.phase, Phase::Starting | Phase::Recording)
        {
            return false;
        }
        match health {
            CaptureHealth::Starting if self.attempt.phase == Phase::Starting => true,
            CaptureHealth::Active | CaptureHealth::Full => {
                if self.attempt.phase == Phase::Starting
                    && (!self.attempt.recording(epoch) || !self.display_capture(cx))
                {
                    return false;
                }
                if health == CaptureHealth::Full {
                    self.stop(cx);
                    return false;
                }
                true
            }
            _ => {
                let lease = self.lease.take();
                self.cancel(cx);
                self.lease = lease;
                self.error("Microphone interrupted. Check your input device and permissions, then try again.".into(), cx);
                false
            }
        }
    }
    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.attempt.phase == Phase::Starting {
            self.cancel(cx);
            return;
        }
        if self.attempt.phase != Phase::Recording {
            return;
        }
        // A stop can race the periodic device-health poll. Never transcribe
        // buffered audio from a lost/replaced input in that interval.
        if !matches!(
            CaptureHealth::from(unsafe { vorkspace_dictation_health() }),
            CaptureHealth::Active | CaptureHealth::Full
        ) {
            self.cancel(cx);
            self.error(
                "Microphone interrupted. Check the selected input and try again.".into(),
                cx,
            );
            return;
        }
        let Some(selected) = self.capture_model else {
            return;
        };
        let Some(epoch) = self.attempt.stop() else {
            return;
        };
        let _ = (selected, epoch);
        if let Err(e) = self.drain(true).and_then(|_| self.finish_input()) {
            self.fail_turn(e, cx);
            return;
        }
        self.status = "Finishing locally… · ⌘, to cancel".into();
        self.publish(cx);
        self.display("Finishing…", panel::State::Transcribing, cx);
    }
    fn drain(&self, stop: bool) -> Result<(), String> {
        thread_local! { static PCM: std::cell::RefCell<Vec<f32>> = const { std::cell::RefCell::new(Vec::new()) }; }
        extern "C" fn receive(p: *const f32, n: usize) {
            if n > 0 && !p.is_null() {
                PCM.with(|v| {
                    *v.borrow_mut() = unsafe { std::slice::from_raw_parts(p, n) }.to_vec()
                });
            }
        }
        let code = if stop {
            unsafe { vorkspace_dictation_stop(receive) }
        } else {
            unsafe { vorkspace_dictation_drain(receive) };
            0
        };
        let pcm = PCM.with(|v| std::mem::take(&mut *v.borrow_mut()));
        if code != 0 {
            return Err(native_error(code).into());
        }
        self.feed_audio(&pcm)
    }
    fn fail_turn(&mut self, error: String, cx: &mut Context<Self>) {
        let lease = self.lease.take();
        self.cancel(cx);
        self.lease = lease;
        self.error(error, cx);
    }
    fn retire_worker(&mut self) {
        self.warm = None;
        self.worker_cancel.cancel();
        self.ready = false;
    }
    fn prepare_worker(&mut self, cx: &mut Context<Self>) {
        self.ready = false;
        let Ok(path) = self.model.clone() else {
            return;
        };
        if self.worker.running {
            return;
        }
        #[cfg(debug_assertions)]
        let admission = self
            .fixture_engine_lock
            .as_deref()
            .map_or_else(EngineLease::acquire, EngineLease::acquire_at);
        #[cfg(not(debug_assertions))]
        let admission = EngineLease::acquire();
        let engine_lease = match admission {
            Ok(lease) => lease,
            Err(AcquireError::Busy) => {
                let status = "Waiting for another Vorkspace dictation model. Disable dictation in that helper; retrying automatically.";
                if self.status != status {
                    self.status = status.into();
                    self.publish(cx);
                }
                if !self.retry_pending {
                    self.retry_pending = true;
                    let epoch = self.retry_epoch;
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        let _ = this.update(cx, |s, cx| {
                            if s.retry_epoch != epoch {
                                return;
                            }
                            s.retry_pending = false;
                            if s.attempt.enabled
                                && !s.worker.running
                                && !s.initializing
                                && s.pending_download.is_none()
                                && s.downloads.id().is_none()
                            {
                                s.prepare_worker(cx);
                            }
                        });
                    })
                    .detach();
                }
                return;
            }
            Err(AcquireError::Unavailable(error)) => {
                self.status = format!(
                    "Could not coordinate dictation models: {error}. Check access to your home folder, then turn dictation off and on to retry."
                );
                self.publish(cx);
                return;
            }
        };
        // This slot belongs to the helper, not a model or connection; it
        // remains occupied until the old thread has actually joined.
        let Some(generation) = self.worker.begin() else {
            return;
        };
        let selected = self.selected;
        self.worker_cancel = CancelToken::new();
        let cancel = self.worker_cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let (events, incoming) = async_channel::bounded(32);
        self.warm = Some(tx);
        self.status = format!("Preparing {} locally…", selected.descriptor().label);
        cx.spawn(async move |this, cx| {
            while let Ok(event) = incoming.recv().await {
                let _ = this.update(cx, |s, cx| s.worker_event(generation, selected, event, cx));
            }
        })
        .detach();
        let (joined_tx, joined_rx) = async_channel::bounded(1);
        let (handle_tx, handle_rx) =
            mpsc::sync_channel::<Result<std::thread::JoinHandle<Result<(), String>>, String>>(1);
        // The joiner owns the lease independently of GPUI entity lifetime. It
        // releases the kernel lock only after the actual model thread joins. It
        // must start first so a failure to create it cannot strand a worker.
        let joiner = std::thread::Builder::new()
            .name("dictation-model-join".into())
            .spawn(move || {
                let result = match handle_rx.recv() {
                    Ok(Ok(handle)) => engine_lease
                        .join(handle)
                        .unwrap_or_else(|_| Err("Local model worker stopped unexpectedly".into())),
                    Ok(Err(error)) => {
                        drop(engine_lease);
                        Err(error)
                    }
                    Err(_) => {
                        drop(engine_lease);
                        Err("Could not start the local model worker".into())
                    }
                };
                let _ = joined_tx.send_blocking(result);
            });
        if let Err(error) = joiner {
            self.worker.joined(generation);
            self.warm = None;
            self.status = format!(
                "Could not start the local model joiner: {error}. Turn dictation off and on to retry."
            );
            self.publish(cx);
            return;
        }
        let handle = std::thread::Builder::new()
            .name("dictation-model".into())
            .spawn(move || worker::run(selected, &path, &cancel, rx, events))
            .map_err(|error| format!("Could not start the local model worker: {error}."));
        // The joiner is already running and waits only on this channel.
        let _ = handle_tx.send(handle);
        cx.spawn(async move |this, cx| {
            let result = joined_rx
                .recv()
                .await
                .unwrap_or_else(|_| Err("Local model worker stopped unexpectedly".into()));
            let _ = this.update(cx, |s, cx| {
                if !s.worker.joined(generation) {
                    return;
                }
                let retired = s.worker_cancel.is_cancelled();
                s.warm = None;
                s.ready = false;
                if s.turn.is_some() {
                    s.cancel(cx);
                    s.turn = None;
                    s.attempt.phase = Phase::Idle;
                    s.lease = None;
                }
                if let Some((id, model)) = s.pending_download.take() {
                    s.start_download(id, model, cx);
                } else if retired && s.attempt.enabled {
                    s.check_ready(cx);
                } else if let Err(e) = result {
                    s.status = format!("{e} Turn dictation off and on to retry.");
                    s.publish(cx);
                }
            });
        })
        .detach();
    }
    fn worker_event(
        &mut self,
        generation: u64,
        selected: ModelId,
        event: Event,
        cx: &mut Context<Self>,
    ) {
        if !self.worker.running || generation != self.worker.generation {
            return;
        }
        // Done also releases a cancelled/retired turn's admission after session drop.
        if let Event::Done(epoch, _) = &event
            && self.turn.as_ref().map(|t| t.0) == Some(*epoch)
            && self.attempt.phase == Phase::Cancelling
        {
            self.turn = None;
            self.attempt.complete(*epoch);
            if self.panel.is_none() {
                self.lease = None;
            }
            return;
        }
        if self.worker_cancel.is_cancelled() || selected != self.selected || !self.attempt.enabled {
            return;
        }
        match event {
            Event::Ready => {
                self.ready = true;
                self.status = "Ready · ⌘, to start and stop".into();
                self.publish(cx);
            }
            Event::Armed(epoch)
                if self.attempt.epoch == epoch && self.attempt.phase == Phase::Starting =>
            {
                #[cfg(debug_assertions)]
                if self.public_audio_fixture {
                    self.attempt.recording(epoch);
                    return;
                }
                let code = self
                    .capture_microphone
                    .as_ref()
                    .and_then(|m| CString::new(m.as_str()).ok())
                    .map_or(8, |m| unsafe {
                        vorkspace_dictation_start(m.as_ptr(), epoch)
                    });
                if code != 0 {
                    self.fail_turn(native_error(code).into(), cx);
                    return;
                }
                self.native_started = true;
                self.display_capture(cx);
            }
            Event::Committed(epoch, text)
                if self.attempt.epoch == epoch
                    && matches!(self.attempt.phase, Phase::Recording | Phase::Transcribing) =>
            {
                if !text.starts_with(&self.pending_text) {
                    self.fail_turn(
                        "Local dictation changed a committed prefix; insertion stopped.".into(),
                        cx,
                    );
                } else {
                    self.pending_text = text;
                    self.deliver(epoch, cx);
                }
            }
            Event::Done(epoch, result) if self.attempt.epoch == epoch => {
                self.finished = Some(result);
                self.deliver(epoch, cx);
            }
            _ => {}
        }
    }
    fn deliver(&mut self, epoch: u64, cx: &mut Context<Self>) {
        if self.attempt.epoch != epoch
            || !matches!(self.attempt.phase, Phase::Recording | Phase::Transcribing)
        {
            return;
        }
        if let Some(Err(e)) = self.finished.take_if(|r| r.is_err()) {
            self.fail_turn(e, cx);
            self.turn = None;
            self.attempt.phase = Phase::Idle;
            return;
        }
        if unsafe { vorkspace_dictation_modifiers_down() } {
            let deadline = *self
                .modifier_deadline
                .get_or_insert_with(|| Instant::now() + Duration::from_secs(2));
            if Instant::now() >= deadline {
                self.fail_turn(native_error(5).into(), cx);
            }
            return;
        }
        self.modifier_deadline = None;
        // Obtain the suffix without consuming it while the previous paste still
        // owns the clipboard. Code 12 means no external effect was attempted.
        let mut next = self.delivered.clone();
        let delta = next.advance(&self.pending_text);
        match delta {
            Err(e) => {
                self.delivered = next;
                self.fail_turn(e, cx);
                return;
            }
            Ok(text) if !text.is_empty() => {
                let code = CString::new(text).map_or(7, |t| unsafe {
                    vorkspace_dictation_paste(t.as_ptr(), epoch, self.delivery)
                });
                if code == 12 {
                    return;
                }
                self.delivered = next;
                self.delivery += 1;
                if code != 0 {
                    self.fail_turn(native_error(code).into(), cx);
                    return;
                }
            }
            _ => {
                self.delivered = next;
            }
        }
        if self.finished.take().is_some() {
            self.turn = None;
            self.capture_model = None;
            self.capture_microphone = None;
            self.attempt.complete(epoch);
            unsafe { vorkspace_dictation_cancel() };
            self.hide(cx);
            self.lease = None;
            self.status = "Ready · ⌘, to start and stop".into();
            self.publish(cx);
        }
    }
}
fn native_error(code: i32) -> &'static str {
    match code {
        1 => {
            "Microphone access is required. Use Set up permissions in Settings → General → Dictation, then enable vorkspace Clipboard in System Settings → Privacy & Security → Microphone."
        }
        2 => {
            "Accessibility access is required for insertion. Use Set up permissions, then enable vorkspace Clipboard in System Settings → Privacy & Security → Accessibility."
        }
        3 => "Could not start the microphone. Check your input device and try again.",
        4 => {
            "No usable destination. Select a text field in a visible window, then start again. Secure fields cannot receive dictation."
        }
        8 => {
            "Selected microphone unavailable. Choose a connected input in Settings → General → Dictation → Microphone."
        }
        9 => {
            "Destination changed while dictating. Keep the same field selected and avoid other input until insertion finishes."
        }
        11 => "Microphone interrupted. Check the selected input and try a new recording.",
        5 => {
            "Release modifier keys before insertion. Further insertion stopped; existing words are kept."
        }
        6 => {
            "Could not preserve the current clipboard. Further insertion stopped; try again with a smaller clipboard."
        }
        _ => "Could not insert dictation. No automatic retry; try a new recording.",
    }
}

pub fn shutdown(cx: &mut App) {
    let s = get(cx);
    let (model, microphone) = (s.read(cx).selected, s.read(cx).microphone.clone());
    configure(false, model, &microphone, "", cx);
    // A posted Cmd-V may not have reached the destination yet. Keep the clipboard
    // available through its normal read window before restoring and terminating.
    cx.spawn(async move |cx| {
        loop {
            let joined = cx.update(|cx| !get(cx).read(cx).worker.running);
            if joined {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
        }
        cx.background_executor()
            .timer(Duration::from_millis(800))
            .await;
        cx.update(|cx| {
            unsafe { vorkspace_dictation_shutdown() };
            cx.quit();
        });
    })
    .detach();
}

#[cfg(test)]
mod model_tests {
    use super::*;
    #[gpui::test]
    fn microphone_change_applies_next_time_without_cancelling_active_attempt(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(crate::Helper::default());
            init(cx);
            let service = get(cx);
            let epoch = service.update(cx, |s, _| {
                // This control-only fixture owns no model worker. Retire init's
                // pending read before enabling its synthetic active attempt.
                s.verification += 1;
                s.model = Err("control-only fixture".into());
                s.attempt.enabled = true;
                s.capture_model = Some(s.selected);
                s.capture_microphone = Some(s.microphone.clone());
                let epoch = s.attempt.begin().unwrap();
                assert!(s.attempt.recording(epoch));
                epoch
            });
            configure(true, ModelId::default(), "uid:next-recording", "", cx);
            service.update(cx, |s, _| {
                assert_eq!(s.microphone, "uid:next-recording");
                assert_eq!(s.attempt.phase, Phase::Recording);
                assert_eq!(s.attempt.epoch, epoch);
                assert_eq!(s.capture_model, Some(ModelId::default()));
                assert_eq!(s.capture_microphone.as_deref(), Some("builtin"));
            });
        });
    }
    #[gpui::test]
    fn initializing_retains_explicit_download_until_verification_and_cancel_removes_it(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(crate::Helper::default());
            init(cx);
            let service = get(cx);
            download(99, ModelId::default(), cx);
            assert_eq!(
                service.read(cx).pending_download,
                Some((99, ModelId::default()))
            );
            assert!(service.read(cx).downloads.id().is_none());
            cancel_model(99, ModelId::default(), cx);
            assert!(service.read(cx).pending_download.is_none());
            assert_eq!(
                service.read(cx).model_notice,
                Some((99, ModelId::default(), ModelState::Cancelled))
            );
        });
    }
    #[gpui::test]
    fn settings_cancel_cancels_acquisition_but_retains_the_worker_slot(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(crate::Helper::default());
            init(cx);
            let service = get(cx);
            service.update(cx, |s, _| {
                assert!(s.downloads.begin(7));
                s.download_cancel = CancelToken::new();
                s.download_model = Some(ModelId::default());
            });
            cancel(cx);
            service.update(cx, |s, _| {
                assert!(s.download_cancel.is_cancelled());
                assert_eq!(s.downloads.id(), Some(7));
                assert!(!s.downloads.accepts(7));
                assert!(!s.downloads.begin(8));
                assert_eq!(s.downloads.finish(7), Some(false));
                assert!(s.downloads.begin(8));
            });
        });
    }
    #[gpui::test]
    fn a_new_enable_verifies_shared_readiness_and_rejects_older_check_results(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(crate::Helper::default());
            init(cx);
            get(cx).update(cx, |s, cx| {
                let old = s.verification;
                s.check_ready(cx);
                let current = s.verification;
                assert!(!s.ready);
                s.checked(old, Ok(true), cx);
                assert!(!s.ready);
                assert!(s.initializing);
                s.checked(current, Ok(true), cx);
                assert!(s.ready);
                assert!(!s.initializing);
            });
        });
    }
    #[gpui::test]
    fn initial_verification_consumes_the_queued_intent_with_the_original_request_id(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(crate::Helper::default());
            init(cx);
            let service = get(cx);
            service.update(cx, |s, _| {
                s.model = Err("isolated model destination unavailable".into())
            });
            download(88, ModelId::default(), cx);
            service.update(cx, |s, cx| {
                s.checked(s.verification, Ok(false), cx);
                assert!(s.pending_download.is_none());
                assert_eq!(
                    s.model_notice,
                    Some((
                        88,
                        ModelId::default(),
                        ModelState::Error("isolated model destination unavailable".into())
                    ))
                );
            });
        });
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[test]
    fn retiring_a_worker_holds_the_engine_slot_until_the_real_join() {
        let mut slot = WorkerSlot::default();
        let first = slot.begin().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (exiting_tx, exiting_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            exiting_tx.send(()).unwrap();
            Ok::<_, &str>(())
        });
        started_rx.recv().unwrap();
        assert!(slot.begin().is_none()); // cancellation or reconfiguration cannot reuse it
        release_tx.send(()).unwrap();
        exiting_rx.recv().unwrap();
        assert!(slot.begin().is_none()); // exiting is not the same as joining
        assert!(worker.join().unwrap().is_ok());
        assert!(!slot.joined(first + 1)); // stale completion cannot release the owner
        assert!(slot.joined(first));
        let second = slot.begin().unwrap();
        let failed = std::thread::spawn(|| Err::<(), _>("synthetic load failure"));
        assert!(failed.join().unwrap().is_err());
        assert!(slot.joined(second));
        assert!(slot.begin().is_some());
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn disabling_a_waiting_helper_fences_its_scheduled_retry(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("engine.lock");
        let owner = EngineLease::acquire_at(&lock).unwrap();
        let service = cx.update(|cx| {
            cx.set_global(crate::Helper::default());
            let service = cx.new(|_| {
                let mut service = Service::new(Ok(dir.path().join("missing.gguf")));
                service.fixture_engine_lock = Some(lock);
                service.initializing = false;
                service.attempt.enabled = true;
                service
            });
            cx.set_global(Dictation(service.clone()));
            service.update(cx, |service, cx| {
                service.prepare_worker(cx);
                assert!(service.retry_pending);
                assert!(!service.worker.running);
                assert!(service.status.contains("retrying automatically"));
            });
            configure(false, ModelId::default(), "builtin", "", cx);
            service
        });
        drop(owner);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        cx.update(|cx| {
            let service = service.read(cx);
            assert!(!service.attempt.enabled);
            assert!(!service.retry_pending);
            assert_eq!(service.worker.generation, 0);
        });
    }

    // No init/check_ready: these production-boundary tests open no microphone,
    // read no model, and schedule no download or transcription worker.
    fn pending(cx: &mut App) -> (Entity<Service>, u64) {
        cx.set_global(crate::Helper::default());
        let service = cx.new(|_| Service {
            #[cfg(debug_assertions)]
            public_audio_fixture: false,
            #[cfg(debug_assertions)]
            fixture_engine_lock: None,
            warm: None,
            worker: WorkerSlot::default(),
            retry_epoch: 0,
            retry_pending: false,
            worker_cancel: CancelToken::new(),
            turn: None,
            native_started: false,
            pending_text: String::new(),
            delivered: CommittedPrefix::default(),
            delivery: 0,
            finished: None,
            modifier_deadline: None,
            attempt: Attempt {
                enabled: true,
                ..Default::default()
            },
            lease: None,
            hotkey: None,
            shortcut_error: None,
            cancel: CancelToken::new(),
            download_cancel: CancelToken::new(),
            downloads: DownloadSlot::default(),
            pending_download: None,
            selected: ModelId::default(),
            microphone: "builtin".into(),
            words: Ok(Vec::new()),
            capture_model: Some(ModelId::default()),
            capture_microphone: Some("builtin".into()),
            download_model: None,
            model_notice: None,
            initializing: false,
            verification: 0,
            ready: true,
            status: String::new(),
            model: Err("capture-only control fixture must not load a model".into()),
            panel: None,
        });
        let epoch = service.update(cx, |s, cx| {
            let epoch = s.attempt.begin().unwrap();
            assert!(s.display_capture(cx));
            epoch
        });
        (service, epoch)
    }

    #[gpui::test]
    fn shared_turn_input_preserves_epoch_chunk_order_finish_and_cancellation(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let (service, epoch) = pending(cx);
            service.update(cx, |s, cx| {
                let (tx, rx) = mpsc::sync_channel(1);
                s.warm = Some(tx);
                s.worker.running = true;
                s.words = vorkspace_core::dictation_words::parse("work space => Vorkspace");
                assert!(s.begin_turn(epoch + 1).is_err());
                s.begin_turn(epoch).unwrap();
                assert!(s.finish_input().is_err());
                let turn = rx.try_recv().unwrap();
                assert_eq!(turn.epoch, epoch);
                s.words = vorkspace_core::dictation_words::parse("work space => Workspace");
                assert_eq!(turn.words[0].preferred, "Vorkspace");
                s.feed_audio(&vec![0.25; worker::SAMPLE_RATE + 8]).unwrap();
                assert!(s.attempt.recording(epoch));
                assert_eq!(s.attempt.stop(), Some(epoch));
                s.finish_input().unwrap();
                let mut received = Vec::new();
                while let worker::Input::Audio(audio) = turn.input.try_recv().unwrap() {
                    received.extend(audio);
                }
                assert_eq!(received, vec![0.25; worker::SAMPLE_RATE + 8]);
                assert!(!s.native_started);
                s.cancel(cx);
                assert!(turn.cancel.is_cancelled());
                s.worker_event(0, s.selected, Event::Done(epoch, Ok(())), cx);
                assert_eq!(s.attempt.phase, Phase::Idle);
            });
        });
    }
    #[gpui::test]
    fn active_turn_cannot_retire_weights_to_start_a_download(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let (service, _) = pending(cx);
            service.update(cx, |s, cx| {
                s.worker.running = true;
                s.start_download(44, s.selected, cx);
                assert!(!s.worker_cancel.is_cancelled());
                assert!(s.downloads.id().is_none());
                assert!(s.pending_download.is_none());
                assert!(matches!(
                    s.model_notice,
                    Some((44, _, ModelState::Error(_)))
                ));
                s.worker.running = false;
                s.cancel(cx);
            });
        });
    }
    #[gpui::test]
    fn superseded_worker_ready_cannot_publish_readiness(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let (service, _) = pending(cx);
            service.update(cx, |s, cx| {
                s.ready = false;
                s.worker.generation = 2;
                s.worker_event(1, s.selected, Event::Ready, cx);
                assert!(!s.ready);
                // A delayed Ready from a worker that already joined cannot
                // make a dead worker look ready during reconnect or failure.
                s.worker_event(2, s.selected, Event::Ready, cx);
                assert!(!s.ready);
                s.worker.running = true;
                s.worker_cancel.cancel();
                s.worker_event(2, s.selected, Event::Ready, cx);
                assert!(!s.ready);
                s.cancel(cx);
            });
        });
    }
    #[gpui::test]
    fn partial_delivery_cancel_and_model_switch_fence_stale_output(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let (service, epoch) = pending(cx);
            service.update(cx, |s, cx| {
                let (pipe, _rx) = Pipe::new();
                s.turn = Some((epoch, pipe));
                s.worker.running = true;
                s.attempt.recording(epoch);
                s.delivered.advance("synthetic prefix").unwrap();
                s.pending_text = "synthetic prefix".into();
                s.cancel(cx);
                assert_eq!(s.attempt.phase, Phase::Cancelling);
                assert!(s.attempt.begin().is_none());
                s.selected = ModelId::ParakeetTdtV3;
                s.worker_event(
                    0,
                    ModelId::UnifiedEnglish,
                    Event::Committed(epoch, "synthetic prefix stale".into()),
                    cx,
                );
                assert!(s.pending_text.is_empty());
                s.worker_event(0, ModelId::UnifiedEnglish, Event::Done(epoch, Ok(())), cx);
                assert_eq!(s.attempt.phase, Phase::Idle);
                assert!(s.turn.is_none());
                assert_eq!(s.delivered.advance("synthetic prefix").unwrap(), "");
            });
        });
    }
    #[gpui::test]
    fn cancel_after_worker_done_releases_admission_without_waiting_for_another_done(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let (service, epoch) = pending(cx);
            service.update(cx, |s, cx| {
                let (pipe, _rx) = Pipe::new();
                s.turn = Some((epoch, pipe));
                s.finished = Some(Ok(()));
                s.cancel(cx);
                assert_eq!(s.attempt.phase, Phase::Idle);
                assert!(s.turn.is_none());
                assert!(s.attempt.begin().is_some());
            });
        });
    }

    #[gpui::test]
    fn pending_becomes_active_and_red_only_on_native_readiness(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let (service, epoch) = pending(cx);
            service.update(cx, |s, cx| {
                assert_eq!(s.attempt.phase, Phase::Starting);
                assert_eq!(s.status, "Starting microphone… · ⌘, to cancel");
                let panel = s.panel.unwrap();
                assert_eq!(panel.read(cx).unwrap().state, panel::State::Starting);
                assert!(!panel.read(cx).unwrap().state.recording());
                assert!(!panel.read(cx).unwrap().state.error());
                assert!(s.capture_health(epoch, CaptureHealth::from(3), cx));
                assert_eq!(s.attempt.phase, Phase::Starting);
                assert_eq!(panel.read(cx).unwrap().state, panel::State::Starting);
                assert!(s.capture_health(epoch, CaptureHealth::from(0), cx));
                assert_eq!(s.attempt.phase, Phase::Recording);
                assert_eq!(s.status, "Recording · ⌘, to finish");
                assert!(panel.read(cx).unwrap().state.recording());
                assert_eq!(panel.read(cx).unwrap().status, "Recording");
                assert_eq!(s.attempt.epoch, epoch);
                assert!(s.capture_health(epoch, CaptureHealth::Active, cx));
                assert_eq!(s.capture_model, Some(s.selected));
                s.cancel(cx);
            });
        });
    }

    #[gpui::test]
    fn startup_toggle_cancels_without_error_or_model_admission(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let (service, epoch) = pending(cx);
            service.update(cx, |s, cx| {
                s.toggle(cx); // The actual shortcut control path.
                assert_eq!(s.attempt.phase, Phase::Idle);
                assert!(s.capture_model.is_none());
                assert!(s.cancel.is_cancelled());
                assert!(s.panel.is_none());
                assert!(s.lease.is_none());
                assert_eq!(s.status, "Cancelled · ⌘, to start again");
                assert!(s.attempt.stop().is_none()); // No inference admission.
                assert!(!s.capture_health(epoch, CaptureHealth::Active, cx));
                assert!(!s.capture_health(epoch, CaptureHealth::Failed, cx));
                assert_eq!(s.status, "Cancelled · ⌘, to start again");
                let new_epoch = s.attempt.begin().unwrap();
                assert_ne!(new_epoch, epoch);
                assert!(s.display_capture(cx));
                assert!(!s.capture_health(epoch, CaptureHealth::Active, cx));
                assert_eq!(s.attempt.phase, Phase::Starting);
                assert_eq!(
                    s.panel.unwrap().read(cx).unwrap().state,
                    panel::State::Starting
                );
                s.stop(cx); // Defensive early-stop path is cancellation too.
                assert_eq!(s.status, "Cancelled · ⌘, to start again");
                assert_eq!(s.attempt.phase, Phase::Idle);
            });
        });
    }

    #[gpui::test]
    fn native_timeout_failure_and_active_loss_remain_errors(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            // Native deadline/route/conversion failures all publish Failed.
            // The Swift policy suite controls the deadline deterministically.
            for (active, health) in [
                (false, CaptureHealth::Failed),
                (false, CaptureHealth::from(-1)),
                (true, CaptureHealth::Failed),
                (true, CaptureHealth::Starting),
            ] {
                let (service, epoch) = pending(cx);
                service.update(cx, |s, cx| {
                    if active {
                        assert!(s.capture_health(epoch, CaptureHealth::Active, cx));
                    }
                    assert!(!s.capture_health(epoch, health, cx));
                    assert_eq!(s.attempt.phase, Phase::Idle);
                    assert!(s.capture_model.is_none());
                    assert!(s.status.starts_with("Microphone interrupted."));
                    assert_eq!(
                        s.panel.unwrap().read(cx).unwrap().state,
                        panel::State::Error
                    );
                    s.cancel(cx);
                });
            }
        });
    }
}
