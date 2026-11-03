//! The helper owns the shortcut and notch. Swift owns the complete speech turn.
use crate::dictation_panel::{self as panel, Panel};
use gpui::{App, AppContext, Context, Entity, Global, Task, WindowHandle};
use std::{
    ffi::{CStr, CString, c_char},
    sync::OnceLock,
    time::Duration,
};
use vorkspace_clipboard::{DictationConfig, DictationPhase, DictationStatus, Notice, word_rules};
use vorkspace_macos::{
    global_hotkey::{self, Registration},
    voice_lease::Lease,
};

unsafe extern "C" {
    fn vorkspace_dictation_configure(json: *const c_char, callback: extern "C" fn(*const c_char));
    fn vorkspace_dictation_toggle();
    fn vorkspace_dictation_setup();
    fn vorkspace_dictation_cancel();
    fn vorkspace_dictation_level() -> f32;
}
static NOTICES: OnceLock<async_channel::Sender<DictationStatus>> = OnceLock::new();
extern "C" fn notice(json: *const c_char) {
    let json = unsafe { CStr::from_ptr(json) }.to_bytes();
    if let Ok(status) = serde_json::from_slice(json)
        && let Some(sender) = NOTICES.get()
    {
        let _ = sender.try_send(status);
    }
}
struct Dictation(Entity<Service>);
impl Global for Dictation {}
#[derive(Default)]
struct Service {
    config: DictationConfig,
    status: DictationStatus,
    shortcut_error: Option<String>,
    hotkey: Option<(Registration, Task<()>)>,
    lease: Option<Lease>,
    panel: Option<WindowHandle<Panel>>,
    meter: Option<Task<()>>,
    presentation: u64,
}

pub fn init(cx: &mut App) {
    let service = cx.new(|_| Service::default());
    cx.set_global(Dictation(service.clone()));
    let (sender, receiver) = async_channel::unbounded();
    let _ = NOTICES.set(sender);
    service.update(cx, |_, cx| {
        cx.spawn(async move |this, cx| {
            while let Ok(status) = receiver.recv().await {
                let _ = this.update(cx, |service, cx| service.notice(status, cx));
            }
        })
        .detach();
    });
}
fn get(cx: &App) -> Entity<Service> {
    cx.global::<Dictation>().0.clone()
}
pub fn status(cx: &App) {
    crate::notify(&Notice::Dictation(get(cx).read(cx).status.clone()), cx);
}

pub fn configure(config: DictationConfig, cx: &mut App) {
    get(cx).update(cx, |s, cx| {
        if s.config == config {
            s.publish(cx);
            return;
        }
        let shortcut_changed =
            s.config.shortcut != config.shortcut || s.config.enabled != config.enabled;
        s.config = config;
        s.shortcut_error = None;
        if shortcut_changed {
            s.hotkey = None;
            if s.config.enabled && !s.config.shortcut.is_empty() {
                match Registration::new(&s.config.shortcut) {
                    Ok((registration, receiver)) => {
                        let task = cx.spawn(async move |this, cx| {
                            while receiver.recv().await.is_ok() {
                                let _ = this.update(cx, |s, cx| s.toggle(cx));
                            }
                        });
                        s.hotkey = Some((registration, task));
                    }
                    Err(error) => s.shortcut_error = Some(error),
                }
            }
        }
        let rules = word_rules(&s.config.words);
        if let Err(error) = &rules {
            s.shortcut_error = Some(error.clone());
        }
        let (key, modifiers) = global_hotkey::parse(&s.config.shortcut).unwrap_or((0, 0));
        let configuration = serde_json::json!({
            "enabled": s.config.enabled && rules.is_ok(), "microphone": s.config.microphone,
            "keyCode": key, "modifiers": modifiers, "rules": rules.unwrap_or_default(),
        });
        let json = CString::new(configuration.to_string()).unwrap();
        unsafe { vorkspace_dictation_configure(json.as_ptr(), notice) };
        s.publish(cx);
    });
}
pub fn setup(_: &mut App) {
    unsafe { vorkspace_dictation_setup() }
}
pub fn toggle(cx: &mut App) {
    get(cx).update(cx, |s, cx| s.toggle(cx));
}
pub fn cancel(cx: &mut App) {
    unsafe { vorkspace_dictation_cancel() };
    // A notch click runs inside that window's event dispatch. Close it after
    // GPUI has released the current window lease.
    cx.spawn(async move |cx| {
        cx.update(|cx| {
            get(cx).update(cx, |s, cx| {
                if !s.status.active {
                    s.close(cx);
                }
            })
        });
    })
    .detach();
}

pub fn shutdown(cx: &mut App) {
    unsafe { vorkspace_dictation_cancel() };
    get(cx).update(cx, |s, cx| s.close(cx));
    cx.quit();
}
impl Service {
    fn publish(&self, cx: &App) {
        crate::notify(&Notice::Dictation(self.status.clone()), cx);
    }
    fn toggle(&mut self, cx: &mut Context<Self>) {
        if !self.config.enabled {
            return;
        }
        if self.lease.is_none() {
            match Lease::acquire() {
                Ok(lease) => self.lease = Some(lease),
                Err(message) => {
                    self.status = DictationStatus {
                        phase: DictationPhase::Error,
                        message,
                        active: false,
                    };
                    self.publish(cx);
                    return;
                }
            }
        }
        unsafe { vorkspace_dictation_toggle() };
    }
    fn notice(&mut self, mut status: DictationStatus, cx: &mut Context<Self>) {
        if !status.active
            && let Some(message) = &self.shortcut_error
        {
            status.phase = DictationPhase::Error;
            status.message = message.clone();
        }
        self.status = status;
        self.publish(cx);
        if self.status.active {
            self.show(cx);
        } else if self.lease.is_some()
            && matches!(
                self.status.phase,
                DictationPhase::Error
                    | DictationPhase::Microphone
                    | DictationPhase::Accessibility
                    | DictationPhase::Model
                    | DictationPhase::Unavailable
            )
        {
            self.show(cx);
            let presentation = self.presentation;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(4)).await;
                let _ = this.update(cx, |s, cx| {
                    if s.presentation == presentation && !s.status.active {
                        s.close(cx);
                    }
                });
            })
            .detach();
        } else {
            self.close(cx);
        }
    }
    fn show(&mut self, cx: &mut Context<Self>) {
        self.presentation = self.presentation.wrapping_add(1);
        if self.panel.is_none() {
            match panel::open(self.status.clone(), cx) {
                Ok(panel) => self.panel = Some(panel),
                Err(_) => {
                    unsafe { vorkspace_dictation_cancel() };
                    return;
                }
            }
        }
        if let Some(panel) = self.panel {
            let _ = panel.update(cx, |p, _, cx| {
                p.status = self.status.clone();
                cx.notify();
            });
        }
        if self.meter.is_none() {
            self.meter = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(40))
                        .await;
                    if this
                        .update(cx, |s, cx| {
                            let Some(panel) = s.panel else { return false };
                            let level = if s.status.phase == DictationPhase::Listening {
                                unsafe { vorkspace_dictation_level() }.clamp(0., 1.)
                            } else {
                                0.
                            };
                            let _ = panel.update(cx, |p, _, cx| {
                                let next = p.level + (level - p.level) * 0.45;
                                if (next - p.level).abs() > 0.005 {
                                    p.level = next;
                                    cx.notify();
                                }
                            });
                            true
                        })
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    break;
                }
            }));
        }
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        self.presentation = self.presentation.wrapping_add(1);
        self.meter = None;
        if let Some(panel) = self.panel.take() {
            let _ = panel.update(cx, |p, window, _| {
                p.placement.close();
                window.remove_window();
            });
        }
        self.lease = None;
    }
}
