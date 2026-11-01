mod controller;
mod host;
mod native;
mod notch;
#[cfg(test)]
mod notch_tests;
#[cfg(debug_assertions)]
pub(crate) mod preview;
pub(crate) use controller::{configure, error as shortcut_error, toggle};
mod web;
use crate::shell::Shell;
use gpui::{Context, WeakEntity, Window, div, prelude::*, px};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::theme::*;

pub(crate) struct Vox {
    owner: WeakEntity<Shell>,
    store: Database,
    host: Option<host::Host>,
    generation: u64,
    active: Arc<AtomicBool>,
    task: Option<gpui::Task<()>>,
    web: Option<web::Web>,
    panel: Option<gpui::WindowHandle<Self>>,
    notch: notch::Presentation,
    idle_task: Option<gpui::Task<()>>,
    screen_watch: Option<native::ScreenChanges>,
    screen_task: Option<gpui::Task<()>>,
    enabled: bool,
    silent: bool,
    microphone: Value,
    cwd: String,
    state: String,
    error: Option<String>,
    reply: String,
    reply_id: String,
    dismissed: Option<String>,
    reply_input: Option<gpui::Entity<crate::input::Input>>,
    reply_truncated: bool,
    face: String,
    receipts: HashMap<String, (String, Result<Value, String>)>,
    pub(crate) devices: Vec<(String, String)>,
}
#[derive(Default)]
struct Calls(Vec<WeakEntity<Vox>>);
impl gpui::Global for Calls {}
pub(crate) fn register(
    voice: &gpui::Entity<Vox>,
    prefs: &crate::settings::Preferences,
    cx: &mut gpui::App,
) {
    configure(prefs, cx);
    if !cx.has_global::<Calls>() {
        cx.set_global(Calls::default());
    }
    cx.global_mut::<Calls>().0.retain(|v| v.upgrade().is_some());
    cx.global_mut::<Calls>().0.push(voice.downgrade());
}
pub(crate) fn shutdown(cx: &mut gpui::App) -> gpui::Task<()> {
    let voices = cx
        .try_global::<Calls>()
        .map(|v| {
            v.0.iter()
                .filter_map(WeakEntity::upgrade)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for voice in &voices {
        voice.update(cx, |v, cx| v.stop(cx));
    }
    cx.spawn(async move |cx| {
        for _ in 0..80 {
            let busy = cx.update(|cx| voices.iter().any(|v| v.read(cx).running()));
            if !busy {
                break;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
        }
        cx.update(|cx| {
            for voice in &voices {
                voice.update(cx, |v, cx| v.finish(cx));
            }
        });
    })
}
impl Vox {
    fn in_call(&self) -> bool {
        self.running() || self.active.load(Ordering::Relaxed)
    }

    pub(super) fn toggle_call(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.enabled {
            self.stop(cx);
            return;
        }
        if self.in_call() {
            self.stop(cx);
        } else {
            self.notch.reopen(native::Geometry::read());
            self.start(window, cx);
        }
    }

    fn attach_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let voice = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            voice
                .update(cx, |voice, cx| {
                    voice.stop(cx);
                    if voice.running() {
                        false
                    } else {
                        voice.release_panel();
                        true
                    }
                })
                .unwrap_or(true)
        });
        self.observe_screens(cx);
    }

    /// Runs `f` in the notch window. Workspace windows read this entity
    /// too, so its current window can't be trusted for window-bound work
    /// such as mounting the microphone page or checking reply focus.
    fn in_panel<R>(
        this: &WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
        f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) -> R,
    ) -> Option<R> {
        let panel = this.read_with(cx, |voice, _| voice.panel).ok().flatten()?;
        panel.update(cx, f).ok()
    }

    fn release_panel(&mut self) {
        self.notch.placement.close();
        self.panel = None;
        self.screen_watch = None;
        self.screen_task = None;
    }

    fn observe_screens(&mut self, cx: &mut Context<Self>) {
        let (watch, receiver) = native::ScreenChanges::new();
        self.screen_watch = Some(watch);
        self.screen_task = Some(cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                let _ = this.update(cx, |voice, cx| {
                    voice.notch.geometry = native::Geometry::read();
                    voice.notch.reposition();
                    cx.notify();
                });
            }
        }));
    }

    pub(crate) fn set_project(&mut self, project: String) {
        if !self.running() {
            self.cwd = project;
        }
    }

    pub(crate) fn running(&self) -> bool {
        self.host.is_some() || self.state == "connecting"
    }

    pub(crate) fn new(
        owner: WeakEntity<Shell>,
        store: Database,
        prefs: &crate::settings::Preferences,
        cwd: String,
    ) -> Self {
        Self {
            owner,
            store,
            host: None,
            generation: 0,
            active: Arc::new(AtomicBool::new(false)),
            task: None,
            web: None,
            panel: None,
            notch: notch::Presentation::default(),
            idle_task: None,
            screen_watch: None,
            screen_task: None,
            enabled: prefs.vox_enabled,
            silent: prefs.vox_silent,
            microphone: prefs.vox_microphone.clone(),
            cwd,
            state: "idle".into(),
            error: None,
            reply: String::new(),
            reply_id: String::new(),
            dismissed: None,
            reply_input: None,
            reply_truncated: false,
            face: "vox".into(),
            receipts: HashMap::new(),
            devices: Vec::new(),
        }
    }
    pub(crate) fn preferences(
        &mut self,
        prefs: &crate::settings::Preferences,
        cx: &mut Context<Self>,
    ) {
        configure(prefs, cx);
        self.enabled = prefs.vox_enabled;
        self.silent = prefs.vox_silent;
        self.microphone = prefs.vox_microphone.clone();
        if !self.enabled {
            self.stop(cx);
        } else if let Some(host) = &self.host {
            host.send(json!({"control":"silent","value":self.silent}));
        }
        cx.notify();
    }
    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        self.notch.close();
        self.notch.hearing = false;
        self.notch.activity = "listening".into();
        self.idle_task = None;
        self.active.store(false, Ordering::Relaxed);
        if self.state == "stopping" {
            cx.notify();
            return;
        }
        {
            self.web = None;
        }
        if let Some(host) = &self.host {
            host.send(json!({"control":"stop"}));
            self.state = "stopping".into();
            let generation = self.generation;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(12))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.generation == generation && this.host.is_some() {
                        this.error = Some(
                            "vox stopped; saving the latest speech could not be confirmed".into(),
                        );
                        this.finish(cx);
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }
    fn finish(&mut self, cx: &mut Context<Self>) {
        self.active.store(false, Ordering::Relaxed);
        self.host = None;
        {
            self.web = None;
        }
        self.state = "idle".into();
        self.notch.hearing = false;
        self.notch.activity = "listening".into();
        self.face = "o-o".into();
        if self.error.is_some() && self.enabled && self.owner.upgrade().is_some() {
            self.notch.closing = false;
            self.touch(cx);
        } else {
            self.notch.close();
        }
        cx.notify();
    }
    fn start(&mut self, _window: &Window, cx: &mut Context<Self>) {
        if !self.enabled || self.host.is_some() || self.state == "connecting" {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.active = Arc::new(AtomicBool::new(true));
        self.error = None;
        self.reply.clear();
        self.reply_id.clear();
        self.dismissed = None;
        self.reply_input = None;
        self.reply_truncated = false;
        self.receipts.clear();
        self.state = "connecting".into();
        self.face = "-_-".into();
        self.touch(cx);
        let cwd = self.cwd.clone();
        let lock_path = match locked(&self.store).and_then(|db| {
            db.data_dir()
                .map(|p| p.join("vox.lock"))
                .ok_or("Vox storage is unavailable".into())
        }) {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(error);
                self.finish(cx);
                return;
            }
        };
        let work = cx
            .background_executor()
            .spawn(async move { host::Host::start(&cwd, &lock_path) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let rx = match this.update(cx, |this, cx| {
                if generation != this.generation || !this.active.load(Ordering::Relaxed) {
                    this.state = "idle".into();
                    cx.notify();
                    return None;
                }
                match result {
                    Ok((host, rx)) => {
                        this.host = Some(host);
                        Some(rx)
                    }
                    Err(e) => {
                        this.error = Some(e);
                        this.finish(cx);
                        None
                    }
                }
            }) {
                Ok(Some(rx)) => rx,
                _ => return,
            };
            // A call lives in its notch; once the notch is gone, so is the call.
            while let Ok(frame) = rx.recv().await {
                if Self::in_panel(&this, cx, |this, window, cx| {
                    if this.generation == generation {
                        this.frame(frame, window, cx);
                    }
                })
                .is_none()
                {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    if this.active.load(Ordering::Relaxed) {
                        this.error = Some("vox disconnected; start it again to reconnect".into());
                    }
                    this.finish(cx);
                }
            });
        }));
        cx.notify();
    }
    fn frame(&mut self, frame: Value, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(url) = frame["ready"].as_str() {
            if !self.active.load(Ordering::Relaxed) {
                return;
            }
            match web::Web::new(url, self.active.clone(), window) {
                Ok(web) => self.web = Some(web),
                Err(e) => {
                    self.error = Some(e);
                    self.finish(cx);
                }
            }
            return;
        }
        let id = frame["id"].clone();
        let action = frame["action"].as_str().unwrap_or_default();
        let input = &frame["input"];
        let result = match action {
            "vox.config" => {
                Ok(json!({"cwd":self.cwd,"silent":self.silent,"microphone":self.microphone}))
            }
            "vox.devices" => {
                self.devices = input["devices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(64)
                    .filter_map(|v| Some((v["id"].as_str()?.into(), v["label"].as_str()?.into())))
                    .collect();
                Ok(Value::Null)
            }
            "vox.state" => {
                let state = input["state"].as_str().unwrap_or("error");
                if let Some(error) = input["error"].as_str() {
                    self.error = Some(error.chars().take(500).collect());
                }
                if self.active.load(Ordering::Relaxed) {
                    self.display_activity(input, cx);
                    self.state = state.into();
                    self.face = match input["display"]["activity"].as_str() {
                        Some("thinking") => "p-p",
                        Some("replying") => "vov",
                        _ => match input["display"]["mood"].as_str() {
                            Some("happy") => "vov",
                            Some("focused") => "-o-",
                            Some("wink_left") => "vo-",
                            Some("wink_right") => "-ov",
                            Some("curious") => "o_o",
                            Some("surprised") => "o!o",
                            Some("unsure") => "o.o",
                            Some("concerned") => "v_v",
                            Some("delighted") => "^o^",
                            Some("cheeky") => ">o<",
                            _ => "o-o",
                        },
                    }
                    .into();
                }
                if state == "error" {
                    self.stop(cx);
                }
                if state == "idle" {
                    self.active.store(false, Ordering::Relaxed);
                    self.state = "idle".into();
                    self.face = "vox".into();
                }
                if let Some(text) = input["display"]["lines"]
                    .as_array()
                    .and_then(|v| v.last())
                    .and_then(|v| v["text"].as_str())
                {
                    let previous = self.reply_id.clone();
                    self.reply = text.chars().take(4000).collect();
                    self.reply_id = input["display"]["lines"]
                        .as_array()
                        .and_then(|lines| lines.last())
                        .and_then(|line| line["id"].as_str())
                        .unwrap_or_default()
                        .to_owned();
                    self.reply_truncated = input["display"]["lines"]
                        .as_array()
                        .and_then(|lines| lines.last())
                        .is_some_and(|line| line["truncated"] == true);
                    if self.reply_id != previous {
                        self.completed_reply(cx);
                    }
                }
                cx.notify();
                Ok(Value::Null)
            }
            _ => {
                let request = frame["requestId"].as_str().unwrap_or_default().to_string();
                let signature = format!("{action}:{input}");
                if let Some((old, result)) = self.receipts.get(&request) {
                    if old == &signature {
                        result.clone()
                    } else {
                        Err("Request ID was reused with different input".into())
                    }
                } else if self.receipts.len() >= 144 {
                    Err("Start Vox again to continue".into())
                } else if !self.active.load(Ordering::Relaxed) && action != "vox.conversation.save"
                {
                    Err("Vox stopped before this request was sent".into())
                } else if action.starts_with("memory.") || action.starts_with("vox.conversation.") {
                    let store = self.store.clone();
                    let action = action.to_string();
                    let input = input.clone();
                    let active = self.active.clone();
                    let generation = self.generation;
                    self.receipts.insert(
                        request.clone(),
                        (signature.clone(), Err("Request is already pending".into())),
                    );
                    let work = cx.background_executor().spawn(async move {
                        let db = locked(&store)?;
                        if !active.load(Ordering::Relaxed) && action != "vox.conversation.save" {
                            return Err("Vox stopped before this request was sent".into());
                        }
                        match action.as_str() {
                            "vox.conversation.read" => {
                                Ok(json!(vorkspace_core::voice::conversation(&db)?))
                            }
                            "vox.conversation.save" => {
                                let expected = serde_json::from_value(input["expected"].clone())
                                    .map_err(|_| "Invalid conversation identity")?;
                                vorkspace_core::voice::save(
                                    &db,
                                    &expected,
                                    input["threadId"].as_str().ok_or("Missing thread ID")?,
                                )?;
                                Ok(Value::Null)
                            }
                            _ => vorkspace_core::voice::memory_action(&db, &action, &input),
                        }
                    });
                    cx.spawn(async move |this, cx| {
                        let result = work.await;
                        let _ = this.update(cx, |this, _| {
                            if this.generation == generation {
                                this.receipts.insert(request, (signature, result.clone()));
                                this.respond(id, result);
                            }
                        });
                    })
                    .detach();
                    return;
                } else {
                    let result = self
                        .owner
                        .update_in(cx, |owner, owner_window, cx| {
                            owner.vox_action(action, input, owner_window, cx)
                        })
                        .unwrap_or(Err("Workspace closed".into()));
                    self.receipts.insert(request, (signature, result.clone()));
                    result
                }
            }
        };
        self.respond(id, result);
    }
    fn respond(&self, id: Value, result: Result<Value, String>) {
        if let Some(host) = &self.host {
            host.send(match result {
                Ok(result) => json!({"id":id,"ok":true,"result":result}),
                Err(error) => json!({"id":id,"ok":false,"error":error}),
            });
        }
    }
    fn bubble_message(&self) -> Option<(String, String)> {
        if !self.enabled {
            return None;
        }
        let message = if let Some(error) = &self.error {
            (format!("error:{error}"), error.clone())
        } else if self.silent && !self.reply.is_empty() {
            (
                format!("reply:{}", self.reply_id),
                format!(
                    "{}{}",
                    self.reply,
                    if self.reply_truncated { "…" } else { "" }
                ),
            )
        } else {
            return None;
        };
        (self.dismissed.as_ref() != Some(&message.0)).then_some(message)
    }
    pub(crate) fn dismiss_reply(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((identity, _)) = self.bubble_message() else {
            return false;
        };
        self.dismissed = Some(identity);
        self.reply_input = None;
        cx.notify();
        true
    }
}
impl Drop for Vox {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Relaxed);
        self.notch.placement.close();
    }
}

impl gpui::Render for Vox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_notch(window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Focusable, TestAppContext, VisualTestContext};
    #[gpui::test]
    fn silent_reply_can_be_selected_and_copied_but_not_edited(cx: &mut TestAppContext) {
        let store = Arc::new(std::sync::Mutex::new(
            vorkspace_core::store::Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap(),
        ));
        let window = cx.update(|cx| {
            crate::input::bind(cx);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| {
                    let prefs = crate::settings::Preferences {
                        vox_enabled: true,
                        vox_silent: true,
                        ..Default::default()
                    };
                    let mut voice =
                        Vox::new(WeakEntity::new_invalid(), store, &prefs, "/project".into());
                    voice.active.store(true, Ordering::Relaxed);
                    voice.reply = "Completed reply 🙂".into();
                    voice
                })
            })
            .unwrap()
        });
        cx.refresh().unwrap();
        window
            .update(cx, |voice, window, cx| {
                let reply = voice.reply_input.as_ref().unwrap();
                window.focus(&reply.focus_handle(cx), cx);
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes("cmd-a cmd-c");
        visual.simulate_input("must not replace the reply");
        window
            .update(visual, |voice, _, cx| {
                assert_eq!(
                    voice.reply_input.as_ref().unwrap().read(cx).content,
                    "Completed reply 🙂"
                );
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("Completed reply 🙂")
                );
                assert!(voice.dismiss_reply(cx));
                assert!(voice.reply_input.is_none());
                assert!(voice.active.load(Ordering::Relaxed));
                assert!(voice.bubble_message().is_none());
                voice.reply_id = "reply-2".into();
                assert!(voice.bubble_message().is_some());
            })
            .unwrap();
    }
}
