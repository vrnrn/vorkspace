//! The helper owns the shortcut and notch. Swift owns the complete speech turn.
use crate::dictation_panel::{self as panel, Panel};
use gpui::{App, AppContext, Context, Entity, Global, Task, WindowHandle};
use std::{
    ffi::{CStr, CString, c_char},
    sync::OnceLock,
    time::Duration,
};
use vorkspace_helper::{DictationConfig, DictationPhase, DictationStatus, Notice, word_rules};
use vorkspace_macos::{global_hotkey, voice_lease::Lease};

unsafe extern "C" {
    fn vorkspace_dictation_configure(
        json: *const c_char,
        callback: extern "C" fn(*const c_char),
        event: extern "C" fn(i32, *const c_char),
    );
    fn vorkspace_dictation_key_action(action: i32);
    fn vorkspace_dictation_setup();
    fn vorkspace_dictation_refresh();
    fn vorkspace_dictation_cancel();
    fn vorkspace_dictation_level() -> f32;
}
enum NativeEvent {
    Status(DictationStatus),
    Key(i32),
    Completed(String),
}
static NOTICES: OnceLock<async_channel::Sender<NativeEvent>> = OnceLock::new();
extern "C" fn event(action: i32, text: *const c_char) {
    let event = if action == 3 && !text.is_null() {
        NativeEvent::Completed(
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned(),
        )
    } else {
        NativeEvent::Key(action)
    };
    if let Some(sender) = NOTICES.get() {
        let _ = sender.try_send(event);
    }
}
extern "C" fn notice(json: *const c_char) {
    let json = unsafe { CStr::from_ptr(json) }.to_bytes();
    if let Ok(status) = serde_json::from_slice(json)
        && let Some(sender) = NOTICES.get()
    {
        let _ = sender.try_send(NativeEvent::Status(status));
    }
}
struct Dictation(Entity<Service>);
impl Global for Dictation {}
#[derive(Default)]
struct Service {
    config: DictationConfig,
    status: DictationStatus,
    shortcut_error: Option<String>,
    lease: Option<Lease>,
    panel: Option<WindowHandle<Panel>>,
    meter: Option<Task<()>>,
    presentation: u64,
    history_epoch: Option<u64>,
    audio_active: bool,
}

pub(crate) fn init(cx: &mut App) {
    let service = cx.new(|_| Service::default());
    cx.set_global(Dictation(service.clone()));
    let (sender, receiver) = async_channel::unbounded();
    let _ = NOTICES.set(sender);
    service.update(cx, |_, cx| {
        cx.spawn(async move |this, cx| {
            while let Ok(event) = receiver.recv().await {
                match event {
                    NativeEvent::Status(status) => {
                        let _ = this.update(cx, |s, cx| s.notice(status, cx));
                    }
                    NativeEvent::Key(action) => {
                        let _ = this.update(cx, |s, cx| s.key_action(action, cx));
                    }
                    NativeEvent::Completed(text) => {
                        let _ = this.update(cx, |s, cx| {
                            let epoch = s.history_epoch.take();
                            crate::service::record_dictation(text, epoch, cx);
                        });
                    }
                }
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
        s.config = config;
        s.shortcut_error = None;
        let rules = word_rules(&s.config.words);
        if let Err(error) = &rules {
            s.shortcut_error = Some(error.clone());
        }
        let (key, modifiers, isolated) = match s.config.shortcut.as_str() {
            "fn" => (63, 0, true),
            "right-alt" => (61, 0, true),
            "" => (0, 0, false),
            shortcut => match global_hotkey::parse(shortcut) {
                Ok((key, modifiers)) => (key, modifiers, false),
                Err(error) => { s.shortcut_error = Some(error); (0, 0, false) }
            },
        };
        let configuration = serde_json::json!({
            "enabled": s.config.enabled && rules.is_ok() && s.shortcut_error.is_none(), "microphone": s.config.microphone,
            "keyCode": key, "modifiers": modifiers, "isolatedKey": isolated, "rules": rules.unwrap_or_default(),
        });
        let json = CString::new(configuration.to_string()).unwrap();
        unsafe { vorkspace_dictation_configure(json.as_ptr(), notice, event) };
        s.publish(cx);
    });
}
pub(crate) fn audio_changed(active: bool, cx: &mut App) {
    get(cx).update(cx, |s, cx| {
        s.audio_active = active;
        if active {
            s.status = DictationStatus {
                phase: DictationPhase::Unavailable,
                message: "Meetings is recording. Dictation is available after it stops.".into(),
                active: false,
            };
            s.publish(cx);
        } else {
            unsafe { vorkspace_dictation_refresh() };
        }
    });
}
pub(crate) fn setup(_: &mut App) {
    unsafe { vorkspace_dictation_setup() }
}
pub(crate) fn toggle(cx: &mut App) {
    get(cx).update(cx, |s, cx| s.toggle(cx));
}
pub(crate) fn cancel(cx: &mut App) {
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

pub(crate) fn shutdown(cx: &mut App) {
    unsafe { vorkspace_dictation_cancel() };
    get(cx).update(cx, |s, cx| s.close(cx));
    cx.quit();
}
impl Service {
    fn publish(&self, cx: &App) {
        crate::notify(&Notice::Dictation(self.status.clone()), cx);
    }
    fn toggle(&mut self, cx: &mut Context<Self>) {
        self.key_action(0, cx);
    }
    fn key_action(&mut self, action: i32, cx: &mut Context<Self>) {
        if !self.config.enabled || self.audio_active {
            return;
        }
        if action != 2 && self.lease.is_none() {
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
        if action != 2 && !self.status.active {
            self.history_epoch = crate::service::history_epoch(cx);
        }
        unsafe { vorkspace_dictation_key_action(action) };
    }
    fn notice(&mut self, mut status: DictationStatus, cx: &mut Context<Self>) {
        if !status.active
            && let Some(message) = &self.shortcut_error
        {
            status.phase = DictationPhase::Error;
            status.message = message.clone();
        }
        if self.audio_active {
            status = DictationStatus {
                phase: DictationPhase::Unavailable,
                message: "Meetings is recording. Dictation is available after it stops.".into(),
                active: false,
            };
        }
        self.status = status;
        if !self.status.active {
            self.history_epoch = None;
        }
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
        let listening = self.status.phase == DictationPhase::Listening;
        if let Some(panel) = self.panel {
            let _ = panel.update(cx, |p, _, cx| {
                p.status = self.status.clone();
                if !listening {
                    p.level = 0.;
                }
                cx.notify();
            });
        }
        if !listening {
            self.meter = None;
        } else if self.meter.is_none() {
            self.meter = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(40))
                        .await;
                    if this
                        .update(cx, |s, cx| {
                            let Some(panel) = s.panel else { return false };
                            let level = unsafe { vorkspace_dictation_level() }.clamp(0., 1.);
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
