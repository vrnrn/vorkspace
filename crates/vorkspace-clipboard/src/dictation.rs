//! Dictation lives in the clipboard process. IPC carries status/control, never PCM or text.
use crate::dictation_panel as panel;
use gpui::{App, AppContext, Context, Entity, Global, Task};
use std::{
    ffi::{CString, c_char},
    path::PathBuf,
    time::{Duration, Instant},
};
use transcribe_cpp::CancelToken;
use vorkspace_clipboard::{ModelState, dictation_model as model, dictation_path};
use vorkspace_clipboard::{
    Notice,
    dictation_state::{Attempt, DownloadSlot, Phase},
};
use vorkspace_macos::{global_hotkey::Registration, voice_lease::Lease};
unsafe extern "C" {
    fn vorkspace_dictation_shutdown();
    fn vorkspace_dictation_start() -> i32;
    fn vorkspace_dictation_stop(callback: extern "C" fn(*const f32, usize));
    fn vorkspace_dictation_cancel();
    fn vorkspace_dictation_health() -> i32;
    fn vorkspace_dictation_permissions();
    fn vorkspace_dictation_paste(text: *const c_char) -> i32;
    fn vorkspace_dictation_modifiers_down() -> bool;
}
struct Dictation(Entity<Service>);
impl Global for Dictation {}
pub struct Service {
    attempt: Attempt,
    lease: Option<Lease>,
    hotkey: Option<(Registration, Task<()>)>,
    shortcut_error: Option<String>,
    cancel: CancelToken,
    download_cancel: CancelToken,
    downloads: DownloadSlot,
    pending_download: Option<u64>,
    model_notice: Option<(u64, ModelState)>,
    initializing: bool,
    verification: u64,
    ready: bool,
    status: String,
    model: Result<PathBuf, String>,
    panel: Option<gpui::WindowHandle<panel::Panel>>,
}
pub fn init(cx: &mut App) {
    let path = dictation_path::model_path();
    let service = cx.new(|_| Service {
        attempt: Attempt::default(),
        lease: None,
        hotkey: None,
        shortcut_error: None,
        cancel: CancelToken::new(),
        download_cancel: CancelToken::new(),
        downloads: DownloadSlot::default(),
        pending_download: None,
        model_notice: None,
        initializing: true,
        verification: 0,
        ready: false,
        status: "Checking the English model…".into(),
        model: path,
        panel: None,
    });
    cx.set_global(Dictation(service.clone()));
    service.update(cx, |s, cx| s.check_ready(cx));
}

fn get(cx: &App) -> Entity<Service> {
    cx.global::<Dictation>().0.clone()
}
pub fn status(cx: &App) {
    get(cx).read(cx).publish(cx)
}
pub fn configure(enabled: bool, cx: &mut App) {
    get(cx).update(cx, |s, cx| {
        if s.attempt.enabled == enabled {
            return;
        }
        s.attempt.enabled = enabled;
        if !enabled {
            s.hotkey = None;
            s.cancel(cx);
            if let Some(id) = s.downloads.id().or(s.pending_download) {
                s.cancel_download(id, cx);
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
        if let Some(id) = s.downloads.id().or(s.pending_download) {
            s.cancel_download(id, cx);
        }
    })
}
pub fn cancel_model(request: u64, cx: &mut App) {
    get(cx).update(cx, |s, cx| s.cancel_download(request, cx));
}
pub fn model_status(request: u64, cx: &mut App) {
    get(cx).update(cx, |s, cx| {
        if let Some((id, state)) = &s.model_notice
            && *id == request
        {
            crate::notify(
                &Notice::Model {
                    request,
                    state: state.clone(),
                },
                cx,
            );
            return;
        }
        // Read-only recovery after reconnect or helper replacement; never replay download.
        let path = s.model.clone();
        cx.spawn(async move |this, cx| {
            let ready = cx
                .background_executor()
                .spawn(async move { path.and_then(|p| model::verify(&p, &CancelToken::new())) })
                .await;
            let _ = this.update(cx, |_, cx| {
                crate::notify(
                    &Notice::Model {
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
pub fn download(request: u64, cx: &mut App) {
    get(cx).update(cx, |s, cx| {
        if s.initializing {
            if s.pending_download.is_some_and(|old| old != request) {
                crate::notify(
                    &Notice::Model {
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
            s.pending_download = Some(request);
            s.model_event(request, ModelState::Checking, cx);
        } else {
            s.start_download(request, cx);
        }
    });
}
impl Service {
    fn check_ready(&mut self, cx: &mut Context<Self>) {
        self.verification += 1;
        let epoch = self.verification;
        let path = self.model.clone();
        self.initializing = true;
        self.ready = false;
        self.status = "Checking the English model…".into();
        self.publish(cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    path.as_ref()
                        .map(|p| model::verified(p))
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
                if r {
                    "Ready · ⌘, to start and stop".into()
                } else {
                    "Download the English model to begin.".into()
                }
            },
        );
        self.publish(cx);
        if let Some(request) = self.pending_download.take() {
            self.start_download(request, cx);
        }
    }

    fn model_event(&mut self, request: u64, state: ModelState, cx: &App) {
        self.model_notice = Some((request, state.clone()));
        crate::notify(&Notice::Model { request, state }, cx);
    }
    fn cancel_download(&mut self, request: u64, cx: &mut Context<Self>) {
        if self.pending_download == Some(request) {
            self.pending_download = None;
            self.model_event(request, ModelState::Cancelled, cx);
        } else if self.downloads.cancel(request) {
            self.download_cancel.cancel();
            self.model_event(request, ModelState::Cancelling, cx);
        } else {
            crate::notify(
                &Notice::Model {
                    request,
                    state: ModelState::Cancelled,
                },
                cx,
            );
        }
    }
    fn start_download(&mut self, request: u64, cx: &mut Context<Self>) {
        if self.pending_download == Some(request) {
            self.pending_download = None;
        }
        if self.downloads.id() == Some(request) {
            if let Some((_, state)) = &self.model_notice {
                crate::notify(
                    &Notice::Model {
                        request,
                        state: state.clone(),
                    },
                    cx,
                );
            }
            return;
        }
        let path = match self.model.clone() {
            Ok(p) => p,
            Err(e) => {
                self.model_event(request, ModelState::Error(e), cx);
                return;
            }
        };
        if self.attempt.phase != Phase::Idle || !self.downloads.begin(request) {
            crate::notify(
                &Notice::Model {
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
        self.download_cancel = CancelToken::new();
        let cancel = self.download_cancel.clone();
        self.model_event(request, ModelState::Downloading(0), cx);
        let (tx, rx) = async_channel::bounded(1);
        cx.spawn(async move |this, cx| {
            while let Ok(bytes) = rx.recv().await {
                let _ = this.update(cx, |s, cx| {
                    if s.downloads.accepts(request) {
                        s.model_event(request, ModelState::Downloading(bytes), cx);
                    }
                });
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    model::download(&path, &cancel, |bytes| {
                        let _ = tx.try_send(bytes);
                    })
                })
                .await;
            let _ = this.update(cx, |s, cx| {
                let Some(publish) = s.downloads.finish(request) else {
                    return;
                };
                let state = if !publish {
                    ModelState::Cancelled
                } else {
                    match result {
                        Ok(()) => {
                            s.verification += 1;
                            s.initializing = false;
                            s.ready = true;
                            ModelState::Ready
                        }
                        Err(e) => ModelState::Error(e),
                    }
                };
                s.model_event(request, state, cx);
                if s.attempt.enabled {
                    s.check_ready(cx);
                } else {
                    s.publish(cx);
                }
                if let Some(pending) = s.pending_download.take() {
                    s.start_download(pending, cx);
                }
            });
        })
        .detach();
    }

    fn publish(&self, cx: &App) {
        crate::notify(
            &Notice::Dictation {
                status: self.shortcut_error.as_ref().map_or_else(
                    || self.status.clone(),
                    |e| format!("{}\n⌘,: {e}", self.status),
                ),
                ready: self.ready,
            },
            cx,
        );
        if let Some((request, state)) = &self.model_notice {
            crate::notify(
                &Notice::Model {
                    request: *request,
                    state: state.clone(),
                },
                cx,
            );
        }
    }
    fn display(&mut self, status: &str, recording: bool, cx: &mut Context<Self>) -> bool {
        if let Some(p) = self.panel {
            if p.update(cx, |p, _, cx| {
                p.status = status.into();
                p.recording = recording;
                cx.notify();
            })
            .is_ok()
            {
                return true;
            }
            self.panel = None;
        }
        self.panel = panel::open(status, recording, cx).ok();
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
        self.display(&self.status.clone(), false, cx);
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
                if self.initializing {
                    self.status =
                        "Checking the English model… Try dictation when verification finishes."
                            .into();
                    self.publish(cx);
                    self.lease = None;
                    return;
                }
                if !self.ready {
                    self.attempt.epoch += 1;
                    self.error(
                        "Download the English model in Settings → Clipboard → Dictation.".into(),
                        cx,
                    );
                    return;
                }
                let Some(epoch) = self.attempt.begin() else {
                    return;
                };
                let result = unsafe { vorkspace_dictation_start() };
                if result != 0 {
                    self.attempt.cancel();
                    self.error(native_error(result).into(), cx);
                    return;
                }
                self.status = "Recording · ⌘, to finish".into();
                self.publish(cx);
                if !self.display("Recording", true, cx) {
                    self.cancel(cx);
                    self.status =
                        "Could not show the recording indicator. Dictation cancelled; try again."
                            .into();
                    self.publish(cx);
                    return;
                }
                self.cancel = CancelToken::new();
                cx.spawn(async move |this, cx| {
                    let started = Instant::now();
                    loop {
                        cx.background_executor().timer(Duration::from_millis(100)).await;
                        let keep = this.update(cx, |s, cx| {
                            if s.attempt.epoch != epoch || s.attempt.phase != Phase::Recording { return false; }
                            let health = unsafe { vorkspace_dictation_health() };
                            if health == 1 {
                                let lease=s.lease.take();
                                s.cancel(cx);
                                s.lease=lease;
                                s.error("Microphone interrupted. Check your input device and permissions, then try again.".into(), cx);
                                return false;
                            }
                            if health == 2 || started.elapsed() >= Duration::from_secs(120) { s.stop(cx); return false; }
                            true
                        }).unwrap_or(false);
                        if !keep { break; }
                    }
                }).detach();
            }
        }
    }
    fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(epoch) = self.attempt.stop() else {
            return;
        };
        // Stop and drain synchronously before inference; no PCM is persisted.
        thread_local! {static PCM:std::cell::RefCell<Vec<f32>>=const {std::cell::RefCell::new(Vec::new())};}
        extern "C" fn receive(p: *const f32, n: usize) {
            if n > 0 && !p.is_null() {
                PCM.with(|v| {
                    *v.borrow_mut() = unsafe { std::slice::from_raw_parts(p, n) }.to_vec()
                });
            }
        }
        unsafe { vorkspace_dictation_stop(receive) };
        let pcm = PCM.with(|v| std::mem::take(&mut *v.borrow_mut()));
        let path = self.model.clone();
        let cancel = self.cancel.clone();
        self.status = "Transcribing locally…".into();
        self.publish(cx);
        self.display("Transcribing…", false, cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { path.and_then(|p| model::transcribe(&p, &pcm, &cancel)) })
                .await;
            // Shortcut modifiers must be released, but cancellation stays responsive.
            for _ in 0..50 {
                if !unsafe { vorkspace_dictation_modifiers_down() } {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
            }
            let _ = this.update(cx, |s, cx| {
                if !s.attempt.complete(epoch) {
                    s.lease = None;
                    return;
                }
                match result {
                    Ok(text) if text.is_empty() => {
                        unsafe { vorkspace_dictation_cancel() };
                        s.status = "No speech detected · ⌘, to try again".into();
                        s.hide(cx);
                        s.lease = None;
                        s.publish(cx);
                    }
                    Ok(text) => {
                        let result = CString::new(text)
                            .map_or(7, |t| unsafe { vorkspace_dictation_paste(t.as_ptr()) });
                        if result != 0 {
                            unsafe { vorkspace_dictation_cancel() };
                            s.error(native_error(result).into(), cx);
                        } else {
                            s.status = "Ready · ⌘, to start and stop".into();
                            s.hide(cx);
                            s.lease = None;
                            s.publish(cx);
                        }
                    }
                    Err(e) => {
                        unsafe { vorkspace_dictation_cancel() };
                        s.error(e, cx);
                    }
                }
            });
        })
        .detach();
    }
}
fn native_error(code: i32) -> &'static str {
    match code {
        1 => {
            "Microphone access is required. Use Set up permissions in Settings → Clipboard, then enable vorkspace Clipboard in System Settings → Privacy & Security → Microphone."
        }
        2 => {
            "Accessibility access is required for insertion. Use Set up permissions, then enable vorkspace Clipboard in System Settings → Privacy & Security → Accessibility."
        }
        3 => "Could not start the microphone. Check your input device and try again.",
        4 => {
            "The original text field is no longer focused or accessible. Keep it selected while dictating and try again."
        }
        5 => "Release modifier keys before insertion. Dictation was discarded; start again.",
        6 => {
            "Could not preserve the current clipboard. Dictation was discarded; try again with a smaller clipboard."
        }
        _ => "Could not insert dictation. No automatic retry; try a new recording.",
    }
}

pub fn shutdown(cx: &mut App) {
    configure(false, cx);
    // A posted Cmd-V may not have reached the destination yet. Keep the clipboard
    // available through its normal read window before restoring and terminating.
    cx.spawn(async move |cx| {
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
    fn initializing_retains_explicit_download_until_verification_and_cancel_removes_it(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(crate::Helper::default());
            init(cx);
            let service = get(cx);
            download(99, cx);
            assert_eq!(service.read(cx).pending_download, Some(99));
            assert!(service.read(cx).downloads.id().is_none());
            cancel_model(99, cx);
            assert!(service.read(cx).pending_download.is_none());
            assert_eq!(
                service.read(cx).model_notice,
                Some((99, ModelState::Cancelled))
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
            download(88, cx);
            service.update(cx, |s, cx| {
                s.checked(s.verification, Ok(false), cx);
                assert!(s.pending_download.is_none());
                assert_eq!(
                    s.model_notice,
                    Some((
                        88,
                        ModelState::Error("isolated model destination unavailable".into())
                    ))
                );
            });
        });
    }
}
