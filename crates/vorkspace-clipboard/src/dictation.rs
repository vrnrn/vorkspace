//! Dictation lives in the clipboard process. IPC carries status/control, never PCM or text.
use crate::{dictation_model as model, dictation_panel as panel};
use gpui::{App, AppContext, Context, Entity, Global, Task};
use std::{
    ffi::{CString, c_char},
    path::PathBuf,
    time::{Duration, Instant},
};
use transcribe_cpp::CancelToken;
use vorkspace_clipboard::{
    Notice,
    dictation_state::{Attempt, Phase},
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
    downloading: bool,
    initializing: bool,
    ready: bool,
    status: String,
    model: PathBuf,
    panel: Option<gpui::WindowHandle<panel::Panel>>,
}
pub fn init(socket: &std::path::Path, cx: &mut App) {
    let directory = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Library/Caches/com.vorkspace.dictation")
        .join(socket.file_stem().unwrap_or_default());
    let service = cx.new(|_| Service {
        attempt: Attempt::default(),
        lease: None,
        hotkey: None,
        shortcut_error: None,
        cancel: CancelToken::new(),
        download_cancel: CancelToken::new(),
        downloading: false,
        initializing: true,
        ready: false,
        status: "Checking the English model…".into(),
        model: directory.join(model::NAME),
        panel: None,
    });
    cx.set_global(Dictation(service.clone()));
    let path = service.read(cx).model.clone();
    cx.spawn(async move |cx| {
        let ready = cx
            .background_executor()
            .spawn(async move {
                model::reap_downloads(&path);
                model::verify(&path, &CancelToken::new()).is_ok()
            })
            .await;
        service.update(cx, |s, cx| {
            s.initializing = false;
            if !ready {
                s.status = "Download the English model to begin.".into();
            }
            if s.downloading {
                return;
            }
            s.ready = ready;
            if ready {
                s.status = "Ready · ⌘, to start and stop".into();
            }
            s.publish(cx);
        });
    })
    .detach();
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
            s.download_cancel.cancel();
            return;
        }
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
    get(cx).update(cx, |s, cx| s.cancel(cx))
}
pub fn download(cx: &mut App) {
    get(cx).update(cx, |s, cx| {
        if !s.attempt.enabled || s.initializing || s.downloading || s.attempt.phase != Phase::Idle {
            return;
        }
        s.downloading = true;
        s.download_cancel = CancelToken::new();
        s.status = "Downloading English model… 0%".into();
        s.publish(cx);
        let cancel = s.download_cancel.clone();
        let path = s.model.clone();
        let (tx, rx) = async_channel::bounded(1);
        cx.spawn(async move |this, cx| {
            while let Ok(bytes) = rx.recv().await {
                let _ = this.update(cx, |s, cx| {
                    if !s.downloading {
                        return;
                    }
                    s.status = if bytes == model::BYTES {
                        "Verifying English model…".into()
                    } else {
                        format!("Downloading English model… {}%", bytes * 100 / model::BYTES)
                    };
                    s.publish(cx);
                });
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let result = model::download(&path, &cancel, |bytes| {
                        let _ = tx.try_send(bytes);
                    });
                    let ready = result.is_ok() || model::verify(&path, &CancelToken::new()).is_ok();
                    (result, ready)
                })
                .await;
            let _ = this.update(cx, |s, cx| {
                s.downloading = false;
                s.ready = result.1;
                s.status = result
                    .0
                    .map_or_else(|e| e, |_| "Ready · ⌘, to start and stop".into());
                s.publish(cx);
            });
        })
        .detach();
    });
}
impl Service {
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
                if self.downloading {
                    self.status = "Wait for the model download to finish.".into();
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
                .spawn(async move { model::transcribe(&path, &pcm, &cancel) })
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
