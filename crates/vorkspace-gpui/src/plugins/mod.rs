//! Native plugin management and isolated custom interfaces share one host boundary.
mod capabilities;
use crate::{
    controls::secondary_button,
    native_view::html::{Html, Reply, RequestHandler},
};
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, Role, Task,
    Window, div, prelude::*, px,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use vorkspace_core::{
    plugins::{self, Plugin},
    provider::Kind,
};
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::theme::*;
#[derive(Clone)]
struct AiScope {
    cwd: String,
    providers: Vec<(Kind, vorkspace_core::accounts::Profile)>,
}
struct HostRequest {
    scope: AiScope,
    owner: String,
    method: String,
    params: Value,
    reply: mpsc::SyncSender<Result<Value, String>>,
}
pub(crate) enum Event {
    OpenConnector(String),
    Disabled(String),
}
pub(crate) struct Page {
    directory: PathBuf,
    cwd: String,
    selected: Option<String>,
    fixed: bool,
    plugins: Vec<Plugin>,
    html: Option<Entity<Html>>,
    error: Option<String>,
    loading: bool,
    focus: FocusHandle,
    generation: u64,
    host: async_channel::Sender<HostRequest>,
    _requests: Task<()>,
    allowed: Vec<Kind>,
    jobs: Vec<(String, Arc<AtomicBool>)>,
}
impl EventEmitter<Event> for Page {}
impl Focusable for Page {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Drop for Page {
    fn drop(&mut self) {
        for (_, cancel) in &self.jobs {
            cancel.store(true, Ordering::Release);
        }
    }
}
impl Page {
    pub(crate) fn new(
        store: Database,
        cwd: String,
        id: Option<&str>,
        hidden: &[String],
        cx: &mut Context<Self>,
    ) -> Self {
        let directory = locked(&store)
            .ok()
            .and_then(|s| s.data_dir().map(PathBuf::from))
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("vorkspace-plugin-memory-{}", std::process::id()))
            });
        let (tx, rx) = async_channel::bounded::<HostRequest>(16);
        let requests = cx.spawn(async move |this, cx| {
            while let Ok(request) = rx.recv().await {
                let fallback = request.reply.clone();
                if this
                    .update(cx, |page, cx| {
                        let reply: Reply = Rc::new(move |result| {
                            let _ = request.reply.try_send(result);
                        });
                        page.host_request(
                            &request.owner,
                            &request.method,
                            request.params,
                            &request.scope,
                            reply,
                            cx,
                        );
                    })
                    .is_err()
                {
                    let _ = fallback.try_send(Err("Plugin view closed".into()));
                }
            }
        });
        let mut page = Self {
            directory,
            cwd,
            selected: id.map(str::to_owned),
            fixed: id.is_some(),
            plugins: vec![],
            html: None,
            error: None,
            loading: false,
            focus: cx.focus_handle(),
            generation: 0,
            host: tx,
            _requests: requests,
            allowed: Kind::ALL
                .into_iter()
                .filter(|k| !hidden.iter().any(|id| id == k.id()))
                .collect(),
            jobs: vec![],
        };
        page.plugins = plugins::list(&page.directory);
        if let Some(id) = page.selected.clone() {
            page.load(&id, cx)
        }
        page
    }
    pub(crate) fn configure_context(&mut self, cwd: &str, hidden: &[String]) {
        self.cwd = cwd.into();
        self.allowed = Kind::ALL
            .into_iter()
            .filter(|kind| !hidden.iter().any(|id| id == kind.id()))
            .collect();
    }
    fn ai_scope(&self, cx: &App) -> AiScope {
        AiScope {
            cwd: self.cwd.clone(),
            providers: self
                .allowed
                .iter()
                .copied()
                .map(|kind| {
                    let profile = crate::accounts::get(cx)
                        .map(|center| center.read(cx).selected(kind, &self.cwd))
                        .unwrap_or_default();
                    (kind, profile)
                })
                .collect(),
        }
    }
    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if !active {
            if let Some(html) = &self.html {
                html.read(cx).hide();
            }
            return;
        }
        self.plugins = plugins::list(&self.directory);
        if let Some(id) = self.selected.clone() {
            if !self
                .plugins
                .iter()
                .any(|p| p.manifest.id == id && p.enabled)
            {
                self.generation += 1;
                self.loading = false;
                self.html = None;
                self.error = Some("This plugin is disabled.".into());
            } else if self.html.is_none() && !self.loading {
                self.load(&id, cx);
            }
        }
        cx.notify();
    }
    pub(crate) fn disabled(&mut self, id: &str, cx: &mut Context<Self>) {
        for (owner, cancel) in &self.jobs {
            if owner == id {
                cancel.store(true, Ordering::Release);
            }
        }
        if self.selected.as_deref() == Some(id) {
            self.generation += 1;
            self.loading = false;
            self.html = None;
            self.error = Some("This plugin is disabled.".into());
        }
        cx.notify();
    }
    fn request(&mut self, owner: &str, value: Value, reply: Reply, cx: &mut Context<Self>) {
        if self.selected.as_deref() != Some(owner) {
            reply(Err("This plugin page was replaced".into()));
            return;
        }
        let Some(method) = value["method"].as_str() else {
            reply(Err("Invalid plugin request".into()));
            return;
        };
        if method.len() > 200 {
            reply(Err("Plugin method is too long".into()));
            return;
        }
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        match value["route"].as_str() {
            Some("host") => {
                let scope = self.ai_scope(cx);
                self.host_request(owner, method, params, &scope, reply, cx)
            }
            Some("plugin") => self.invoke(owner, method, params, reply, cx),
            _ => reply(Err("Unknown plugin request route".into())),
        }
    }
    fn invoke(
        &mut self,
        owner: &str,
        method: &str,
        params: Value,
        reply: Reply,
        cx: &mut Context<Self>,
    ) {
        let descriptor = match plugins::find(&self.directory, owner) {
            Ok(p) => p,
            Err(e) => {
                reply(Err(e));
                return;
            }
        };
        let directory = match vorkspace_plugin::private_directory(&self.directory, owner) {
            Ok(p) => p,
            Err(e) => {
                reply(Err(e));
                return;
            }
        };
        let context = vorkspace_plugin::Context {
            data_dir: directory,
            cwd: PathBuf::from(&self.cwd),
        };
        let method = method.to_owned();
        let scope = self.ai_scope(cx);
        let host = self.host.clone();
        let owner = owner.to_owned();
        let work = cx.background_executor().spawn(async move {
            let timeout = if method == "meetings.summarize" {
                Duration::from_secs(900)
            } else {
                Duration::from_secs(120)
            };
            vorkspace_plugin::call(
                &descriptor.executable,
                Some(&owner),
                &context,
                &method,
                params,
                timeout,
                |method, params| {
                    let (tx, rx) = mpsc::sync_channel(1);
                    host.try_send(HostRequest {
                        scope: scope.clone(),
                        owner: owner.clone(),
                        method: method.into(),
                        params: params.clone(),
                        reply: tx,
                    })
                    .map_err(|_| "Plugin host is busy or closed".to_owned())?;
                    rx.recv_timeout(Duration::from_secs(190)).map_err(|_| {
                        "Plugin host request timed out; it was not retried".to_owned()
                    })?
                },
            )
            .map(|(_, value)| value)
        });
        cx.spawn(async move |_, _| {
            reply(work.await);
        })
        .detach();
    }
    fn load(&mut self, id: &str, cx: &mut Context<Self>) {
        self.generation += 1;
        self.loading = true;
        self.error = None;
        self.html = None;
        self.selected = Some(id.into());
        let generation = self.generation;
        let weak = cx.entity().downgrade();
        let app = cx.to_async();
        let owner = id.to_owned();
        let handler: RequestHandler = Rc::new(move |value, reply| {
            let fallback = reply.clone();
            if weak
                .update(&mut app.clone(), |page, cx| {
                    page.request(&owner, value, reply, cx)
                })
                .is_err()
            {
                fallback(Err("Plugin page closed".into()));
            }
        });
        let weak = cx.entity().downgrade();
        let app = cx.to_async();
        let reply: Reply = Rc::new(move |result| {
            let _ = weak.update(&mut app.clone(), |page, cx| {
                if page.generation != generation {
                    return;
                }
                page.loading = false;
                match result {
                    Ok(value) => match value["html"].as_str() {
                        Some(body) => {
                            page.html = Some(cx.new(|_| Html::plugin(body.into(), handler.clone())))
                        }
                        None => page.error = Some("Plugin returned no interface document".into()),
                    },
                    Err(error) => page.error = Some(error),
                }
                cx.notify();
            });
        });
        self.invoke(id, "ui.document", json!({}), reply, cx);
    }
    fn install(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Install trusted plugin executable".into()),
        });
        let directory = self.directory.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picker.await else {
                return;
            };
            let Some(path) = paths.first().cloned() else {
                return;
            };
            let work = cx
                .background_executor()
                .spawn(async move { plugins::install(&directory, &path) });
            let result = work.await;
            let _ = this.update(cx, |page, cx| {
                match result {
                    Ok(_) => {
                        page.plugins = plugins::list(&page.directory);
                        page.error = None;
                    }
                    Err(e) => page.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(html) = &self.html {
            let mut page = div().size_full().flex().flex_col().track_focus(&self.focus);
            if !self.fixed {
                page = page.child(
                    div().p_3().border_b_1().border_color(content(0.1)).child(
                        secondary_button("plugins-back", "Back to plugins", false, false)
                            .child("Plugins")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.generation += 1;
                                this.html = None;
                                this.selected = None;
                                this.loading = false;
                                this.error = None;
                                this.plugins = plugins::list(&this.directory);
                                cx.notify();
                            })),
                    ),
                );
            }
            return page
                .child(div().flex_1().min_h_0().child(html.clone()))
                .into_any_element();
        }
        let mut root = div()
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .text_color(content(1.))
            .bg(pane_fill())
            .p_6()
            .gap_5();
        root = root.child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(if self.fixed { "Meetings" } else { "Plugins" }),
                )
                .when(!self.fixed, |row| {
                    row.child(
                        secondary_button("install-plugin", "Install plugin", false, false)
                            .child("Install plugin…")
                            .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                    )
                }),
        );
        if let Some(error) = &self.error {
            root = root.child(
                div()
                    .id("plugin-error")
                    .role(Role::Alert)
                    .text_sm()
                    .text_color(crate::controls::red(1.))
                    .child(error.clone()),
            );
        }
        if self.loading {
            return root
                .child(div().h_8().bg(content(0.04)).rounded_md())
                .into_any_element();
        }
        if self.fixed {
            return root
                .child(
                    div()
                        .text_sm()
                        .text_color(content(0.7))
                        .child("Build or reinstall the Meetings plugin, then reopen this page."),
                )
                .into_any_element();
        }
        root=root.child(div().text_sm().text_color(content(0.7)).child("Install local executables you trust. Plugins run as your user; only declared capabilities can use the app’s native audio and AI services."));
        for plugin in self.plugins.clone() {
            let id = plugin.manifest.id.clone();
            let open_id = id.clone();
            let toggle_id = id.clone();
            let available = plugin.executable.is_file();
            let mut row =
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .py_3()
                    .border_b_1()
                    .border_color(content(0.1))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(plugin.manifest.name.clone())
                            .child(div().text_xs().text_color(content(0.7)).child(
                                if plugin.bundled {
                                    "Built in".to_owned()
                                } else {
                                    format!(
                                        "v{} · {}",
                                        plugin.manifest.version,
                                        plugin.manifest.capabilities.join(", ")
                                    )
                                },
                            )),
                    );
            let mut actions = div().flex().gap_2();
            if plugin.manifest.ui {
                actions = actions.child(
                    secondary_button(
                        gpui::ElementId::Name(format!("plugin-open-{id}").into()),
                        "Open plugin",
                        false,
                        !available || !plugin.enabled,
                    )
                    .child("Open")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if available {
                            this.load(&open_id, cx);
                            cx.notify();
                        }
                    })),
                );
            } else if plugin.manifest.connector {
                actions = actions.child(
                    secondary_button(
                        gpui::ElementId::Name(format!("plugin-connector-{id}").into()),
                        "Open connector",
                        false,
                        !available,
                    )
                    .child("Open")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(Event::OpenConnector(open_id.clone()))
                    })),
                );
            }
            if !plugin.manifest.connector {
                let enabled = plugin.enabled;
                actions = actions.child(
                    secondary_button(
                        gpui::ElementId::Name(format!("plugin-toggle-{id}").into()),
                        "Enable or disable plugin",
                        false,
                        false,
                    )
                    .child(if enabled { "Disable" } else { "Enable" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !enabled {
                            this.error =
                                plugins::set_enabled(&this.directory, &toggle_id, true).err();
                        } else {
                            if let Some(state) = crate::clipboard::audio::owned(&toggle_id, cx) {
                                let _ = crate::clipboard::audio::stop(&toggle_id, &state, cx);
                            }
                            this.error =
                                plugins::set_enabled(&this.directory, &toggle_id, false).err();
                            if this.error.is_none() {
                                cx.emit(Event::Disabled(toggle_id.clone()));
                            }
                        }
                        this.plugins = plugins::list(&this.directory);
                        cx.notify();
                    })),
                );
            }
            if !available {
                actions = actions.child(
                    div()
                        .text_xs()
                        .text_color(content(0.5))
                        .child("Not packaged"),
                );
            }
            row = row.child(actions);
            root = root.child(row);
        }
        root.into_any_element()
    }
}
