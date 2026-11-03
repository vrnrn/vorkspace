//! The built-in Meetings document view. Only this bundled page can call its commands.
pub(crate) mod capture;
pub(crate) use capture::stop_active;

use crate::native_view::html::{Html, Reply};
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, Render, Window, div, prelude::*,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use vorkspace_core::{
    accounts::Profile,
    provider::{self, Kind},
};
use vorkspace_meetings::Meetings;
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::theme::*;

/// Meetings send user transcripts to a background process. Read-only
/// agent sandboxes still permit tools, so use explicit no-tools modes.
pub(crate) fn supports_summary(kind: Kind) -> bool {
    kind == Kind::Claude || kind.is_pi()
}

pub(crate) struct Page {
    meetings: Result<Meetings, String>,
    cwd: String,
    hidden: Vec<String>,
    html: Entity<Html>,
    focus: FocusHandle,
    jobs: Vec<Arc<AtomicBool>>,
    enabled: bool,
    enable_epoch: u64,
}

impl Focusable for Page {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        for cancel in &self.jobs {
            cancel.store(true, Ordering::Release);
        }
    }
}

impl Page {
    pub(crate) fn new(store: Database, cx: &mut Context<Self>) -> Self {
        let meetings = locked(&store)
            .ok()
            .and_then(|store| store.data_dir().map(PathBuf::from))
            .ok_or_else(|| "Meeting storage is unavailable.".to_owned())
            .and_then(|root| Meetings::open(&root));
        let weak = cx.entity().downgrade();
        let app = cx.to_async();
        let html = cx.new(|_| {
            Html::meetings(Rc::new(move |value, reply| {
                let fallback = reply.clone();
                if weak
                    .update(&mut app.clone(), |page, cx| {
                        page.request(value, reply, cx);
                    })
                    .is_err()
                {
                    fallback(Err("The Meetings view was closed.".into()));
                }
            }))
        });
        Self {
            meetings,
            cwd: String::new(),
            hidden: Vec::new(),
            html,
            focus: cx.focus_handle(),
            jobs: Vec::new(),
            enabled: true,
            enable_epoch: 0,
        }
    }

    pub(crate) fn configure_context(&mut self, cwd: &str, hidden: &[String]) {
        self.cwd = cwd.into();
        self.hidden = hidden.into();
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if !active {
            self.html.read(cx).hide();
        }
        cx.notify();
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        self.enable_epoch += 1;
        if !enabled {
            self.set_active(false, cx);
            for cancel in &self.jobs {
                cancel.store(true, Ordering::Release);
            }
        }
    }

    fn request(&mut self, mut value: Value, reply: Reply, cx: &mut Context<Self>) {
        if !self.enabled {
            return reply(Err("Meetings is turned off.".into()));
        }
        let meetings = match &self.meetings {
            Ok(meetings) => meetings.clone(),
            Err(error) => return reply(Err(error.clone())),
        };
        let params = value.get_mut("params").map(Value::take).unwrap_or_default();
        let id = params["id"].as_str().unwrap_or_default();
        match value["method"].as_str() {
            Some("audio.current") => reply(Ok(json!(capture::current(cx)))),
            Some("audio.stop") => reply(capture::stop(id, cx)),
            Some("audio.status") => reply(capture::status(
                id,
                params["after"].as_u64().unwrap_or(0),
                meetings.directory,
                cx,
            )),
            Some("meetings.start") => {
                let enable_epoch = self.enable_epoch;
                let directory = meetings.directory.clone();
                let prepare = cx
                    .background_executor()
                    .spawn(async move { meetings.prepare("") });
                cx.spawn(async move |this, cx| {
                    let result = match prepare.await {
                        Ok(id) => this
                            .update(cx, |page, cx| {
                                if page.enable_epoch != enable_epoch {
                                    return Err("Recording start was canceled because Meetings was turned off.".into());
                                }
                                capture::start(id, directory, cx)
                            })
                            .unwrap_or_else(|_| Err("The Meetings view was closed.".into())),
                        Err(error) => Err(error),
                    };
                    reply(result);
                })
                .detach();
            }
            Some("meetings.summarize") => self.summarize(meetings, id.to_owned(), reply, cx),
            Some(method @ ("meetings.list" | "meetings.read" | "meetings.update")) => {
                let method = method.to_owned();
                let work = cx.background_executor().spawn(async move {
                    match method.as_str() {
                        "meetings.list" => meetings.list(),
                        "meetings.read" => meetings.read(params["id"].as_str().unwrap_or_default()),
                        _ => meetings.update(&params),
                    }
                });
                cx.spawn(async move |_, _| reply(work.await)).detach();
            }
            _ => reply(Err("Unknown Meetings command.".into())),
        }
    }

    fn summarize(&mut self, meetings: Meetings, id: String, reply: Reply, cx: &mut Context<Self>) {
        self.jobs.retain(|job| Arc::strong_count(job) > 1);
        if self.jobs.len() >= 4 {
            return reply(Err("Meeting summaries are busy. Try again shortly.".into()));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs.push(cancel.clone());
        let timeout = cancel.clone();
        let deadline = cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(900))
                .await;
            timeout.store(true, Ordering::Release);
        });
        let cwd = self.cwd.clone();
        let profiles: Vec<(Kind, Profile)> = Kind::ALL
            .into_iter()
            .filter(|kind| supports_summary(*kind) && !self.hidden.iter().any(|id| id == kind.id()))
            .map(|kind| {
                let profile = crate::accounts::get(cx)
                    .map(|center| center.read(cx).selected(kind, &cwd))
                    .unwrap_or_default();
                (kind, profile)
            })
            .collect();
        let work = cx.background_executor().spawn(async move {
            let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
            meetings.summarize(&id, &cancel, |settings, prompt| {
                if cancel.load(Ordering::Acquire) {
                    return Err("Meeting summary canceled.".into());
                }
                let (kind, account) = profiles
                    .iter()
                    .find(|(kind, _)| kind.id() == settings.provider)
                    .ok_or("This provider is unavailable in Vorkspace.")?;
                let binary = provider::resolve(*kind)
                    .ok_or("Connect the selected AI provider in Settings first.")?;
                provider::run_text_with_timeout(
                    provider::Request {
                        kind: *kind,
                        binary,
                        cwd: scratch.path().into(),
                        prompt,
                        provider_id: None,
                        model: Some(settings.model.clone()),
                        settings: vorkspace_core::agent::Settings {
                            account: account.clone(),
                            ..Default::default()
                        },
                    },
                    cancel.clone(),
                    Duration::from_secs(180),
                )
                .and_then(|text| {
                    if cancel.load(Ordering::Acquire) {
                        Err("Meeting summary canceled.".into())
                    } else {
                        Ok(text)
                    }
                })
            })
        });
        cx.spawn(async move |_, _| {
            let result = work.await;
            drop(deadline);
            reply(result);
        })
        .detach();
    }
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus)
            .bg(pane_fill())
            .child(self.html.clone())
    }
}
