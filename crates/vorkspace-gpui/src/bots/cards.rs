use super::*;
use serde_json::{Value, json};

/// launchedSessionState: where a session the Bot started stands.
fn launched_state(session: &Conversation, cx: &App) -> &'static str {
    if session.needs_approval() {
        "input"
    } else if session.busy {
        "working"
    } else if !session.summary.has_messages {
        "draft"
    } else {
        match session.external_outcome(cx) {
            (false, status) if status.starts_with("Stopped") => "stopped",
            (false, _) => "failed",
            _ => "done",
        }
    }
}
impl Bots {
    pub(super) fn refresh_cards(&mut self, cx: &mut Context<Self>) {
        let Some(bot) = self.current().cloned() else {
            return;
        };
        let Some(chat) = self.chat() else {
            return;
        };
        let now = vorkspace_core::time::now_ms();
        // Saved summaries keep cards current without opening their providers.
        let summaries = self.owner.session_views(cx);
        // LaunchedSessionCard: each launch's live state, for the chat's cards.
        let launches = bot
            .launches
            .iter()
            .map(|launch| {
                let session = self.owner.session_entity(launch.session_id, cx);
                let view = match session {
                    Some(session) => {
                        let session = session.read(cx);
                        crate::transcript::LaunchView {
                            title: session.summary.title.clone(),
                            state: launched_state(session, cx),
                            harness: session.summary.provider.clone(),
                            cwd: session.summary.cwd.clone(),
                        }
                    }
                    None => {
                        let summary = summaries.iter().find(|s| s.id == launch.session_id);
                        crate::transcript::LaunchView {
                            title: summary
                                .map(|s| s.title.clone())
                                .unwrap_or_else(|| launch.title.clone()),
                            state: summary.map_or("unavailable", |s| {
                                if s.needs_input {
                                    "input"
                                } else if s.busy {
                                    "working"
                                } else if !s.has_messages {
                                    "draft"
                                } else {
                                    "done"
                                }
                            }),
                            harness: launch.harness.clone(),
                            cwd: summary
                                .map(|s| s.cwd.clone())
                                .unwrap_or_else(|| launch.cwd.clone()),
                        }
                    }
                };
                (launch.session_id, view)
            })
            .collect();
        let transcript = chat.read(cx).transcript.clone();
        transcript.update(cx, |t, cx| t.set_launches(launches, cx));
        if chat.read(cx).needs_history_restore() {
            // Card writes require the complete history and their durable CAS.
            // Restore only when this window actually contains cards to refresh;
            // the next scheduler tick will retry after restoration.
            if chat
                .read(cx)
                .bot_rows(cx)
                .iter()
                .any(|row| row.role == "bot-card")
            {
                chat.update(cx, |view, cx| view.load(cx));
            }
            return;
        }
        // Only mutable cards need a snapshot; never clone the conversation's
        // prose or tool output while refreshing a few card states.
        let cards = chat
            .read(cx)
            .bot_rows(cx)
            .iter()
            .enumerate()
            .filter(|(_, r)| r.role == "bot-card")
            .map(|(index, row)| (index, row.text.clone()))
            .collect::<Vec<_>>();
        let generation = chat.read(cx).submission_generation;
        for (index, text) in cards {
            let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let original = value.clone();
            let Some(id) = value["id"].as_str().map(str::to_owned) else {
                continue;
            };
            if value["type"] == "session" {
                if let Some(session) = value["sessionId"]
                    .as_i64()
                    .and_then(|id| self.owner.session_entity(id, cx))
                {
                    let session = session.read(cx);
                    value["title"] = json!(session.summary.title);
                    value["state"] = json!(launched_state(session, cx));
                    value["harness"] = json!(session.summary.provider);
                    value["cwd"] = json!(session.summary.cwd);
                }
            } else if value["type"] == "habit" {
                value["started"] = json!(self.habits.iter().any(|h| h.id == format!("card-{id}")));
            } else if value["type"] == "document" {
                if let Some(document) = bot
                    .documents
                    .iter()
                    .find(|d| Some(d.id.as_str()) == value["documentId"].as_str())
                {
                    value["title"] = json!(document.title);
                    value["kind"] = json!(document.kind);
                    value["summary"] = json!(document.summary);
                }
            } else if value["type"] == "pr" {
                if self
                    .card_refresh
                    .get(&id)
                    .is_some_and(|at| now.saturating_sub(*at) < 60000)
                {
                    continue;
                }
                self.card_refresh.insert(id.clone(), now);
                let cwd = bot.projects.first().cloned().unwrap_or_else(|| ".".into());
                let repo = value["repo"].as_str().map(str::to_owned);
                let number = value["number"].as_u64().unwrap_or(0);
                let view = chat.downgrade();
                let work = cx
                    .background_executor()
                    .spawn(async move { bot::cards::pull_request(&cwd, repo.as_deref(), number) });
                cx.spawn(async move |_, cx| {
                    if let Ok(metadata) = work.await {
                        for key in ["title", "state", "url", "isDraft", "additions", "deletions"] {
                            value[key] = metadata[key].clone();
                        }
                        let _ = view.update(cx, |v, cx| {
                            v.bot_patch_card(index, &id, generation, original, value, cx)
                        });
                    }
                })
                .detach();
                continue;
            }
            let _ = chat.update(cx, |v, cx| {
                v.bot_patch_card(index, &id, generation, original, value, cx)
            });
        }
    }

    pub(super) fn hold_card(&mut self, session: i64, bot: &str, value: Value) {
        if let Some(run) = self.running.get_mut(&session) {
            run.cards.push(value);
        } else if let Some(bot) = self.roster.get_mut(bot) {
            bot.pending_reports.push(format!("bot-card:{value}"));
        }
    }

    pub(super) fn card_action(&mut self, bot: &str, value: &Value, cx: &mut Context<Self>) {
        if value["type"] != "choice" {
            self.active = Some(bot.into());
            if let Some(handle) = self.parent_window {
                let app: &mut App = cx;
                app.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, _| window.activate_window());
                });
            }
        }
        match value["type"].as_str().unwrap_or_default() {
            "choice" => {
                if let Some(text) = value["text"].as_str() {
                    self.enqueue(bot, text.into(), Vec::new(), false, cx);
                }
            }
            "session" => {
                if let Some(id) = value["sessionId"].as_i64() {
                    self.route = Some(Route::Session(id));
                    self.show_overview(cx);
                    self.overview_tab = OverviewTab::Sessions;
                    cx.notify();
                }
            }
            "document" => {
                if let Some(id) = value["documentId"].as_str() {
                    self.open_document(id, cx);
                    self.route = Some(Route::Document(id.into()));
                    self.show_overview(cx);
                    self.overview_tab = OverviewTab::Documents;
                    cx.notify();
                }
            }
            "habit" => {
                if value["started"] == true {
                    self.show_overview(cx);
                    self.overview_tab = OverviewTab::Habits;
                    self.route = Some(Route::Habit(format!(
                        "card-{}",
                        value["id"].as_str().unwrap_or_default()
                    )));
                    cx.notify();
                    return;
                }
                let result = serde_json::from_value::<habits::Schedule>(value["schedule"].clone())
                    .map_err(|e| e.to_string())
                    .and_then(|schedule| {
                        Habit::new(
                            value["name"].as_str().unwrap_or_default().into(),
                            value["instructions"].as_str().unwrap_or_default().into(),
                            schedule,
                            vorkspace_core::time::now_ms(),
                        )
                    });
                match result {
                    Ok(mut habit) => {
                        habit.id = format!("card-{}", value["id"].as_str().unwrap_or_default());
                        let root = self.root.clone();
                        let id = bot.to_string();
                        self.file_work(
                            id.clone(),
                            move || {
                                habits::update(&root, &id, |all| {
                                    if !all.iter().any(|h| h.id == habit.id) {
                                        all.push(habit.clone());
                                    }
                                    Ok(())
                                })
                            },
                            cx,
                        );
                        self.show_overview(cx);
                        self.overview_tab = OverviewTab::Habits;
                        self.route = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            "pr" => {
                if let Some(url) = value["url"]
                    .as_str()
                    .filter(|s| s.starts_with("https://github.com/"))
                {
                    cx.open_url(url);
                } else if let (Some(repo), Some(number)) =
                    (value["repo"].as_str(), value["number"].as_u64())
                {
                    cx.open_url(&format!("https://github.com/{repo}/pull/{number}"));
                }
            }
            _ => (),
        }
        cx.notify();
    }
}
