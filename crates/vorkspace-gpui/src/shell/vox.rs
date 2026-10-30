use super::*;
use serde_json::{Value, json};
impl Shell {
    pub(crate) fn toggle_vox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace.preferences.vox_enabled {
            self.open_settings(crate::settings::Section::Vox, window, cx);
            return;
        }
        if let Some(voice) = self.vox.clone() {
            voice.update(cx, |voice, _| {
                voice.set_project(self.workspace.project.clone())
            });
            crate::vox::toggle(voice, cx);
        }
    }

    pub(super) fn vox_launcher(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let active = self
            .vox
            .as_ref()
            .is_some_and(|voice| voice.read(cx).running());
        let label = if active {
            "End Vox call"
        } else if self.workspace.preferences.vox_enabled {
            "Start Vox"
        } else {
            "Enable Vox"
        };
        crate::controls::button("vox-launcher", label, true)
            .tooltip(move |_, cx| cx.new(|_| crate::controls::Tooltip(label.into())).into())
            .debug_selector(|| "vox-launcher".into())
            .h(px(32.))
            .min_w_0()
            .rounded_md()
            .flex()
            .items_center()
            .gap_2()
            .when(compact, |d| d.w(px(32.)).justify_center())
            .when(!compact, |d| d.w_full().px_2())
            .text_color(content(if active { 1. } else { 0.65 }))
            .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
            .child(icon("mic", 14.))
            .when(!compact, |d| {
                d.child(div().text_size(px(12.)).flex_1().child("Vox"))
            })
            .when(active, |d| {
                d.child(div().size(px(4.)).rounded_full().bg(content(0.7)))
            })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_vox(window, cx)))
            .into_any_element()
    }

    pub(crate) fn vox_action(
        &mut self,
        action: &str,
        input: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        validate_voice_input(action, input)?;
        let projects = self
            .workspace
            .projects
            .iter()
            .filter(|p| vorkspace_core::remote::parse_remote_path(p).is_none())
            .cloned()
            .collect::<Vec<_>>();
        let bots = self
            .bots
            .as_ref()
            .map(|m| m.read(cx).roster.clone())
            .unwrap_or_default();
        if action == "workspace.current" {
            let selected = self
                .bots
                .as_ref()
                .filter(|_| self.bot_active(cx))
                .and_then(|m| m.read(cx).chat())
                .or_else(|| {
                    self.workspace.active.and_then(|id| {
                        self.sessions
                            .get(&self.workspace.session_for_pane(id))
                            .cloned()
                    })
                });
            return Ok(
                json!({"session":selected.map(|s|{let s=s.read(cx);json!({"id":s.summary.id,"title":s.summary.title,"cwd":s.summary.cwd,"harness":s.summary.provider,"busy":s.busy})}),"projects":projects,"bots":bots.bots.iter().map(|b|json!({"id":b.id,"name":b.label()})).collect::<Vec<_>>()}),
            );
        }
        if action == "bots.list" {
            return Ok(json!(
                bots.bots
                    .iter()
                    .map(|b| json!({"id":b.id,"name":b.label(),"projects":b.projects}))
                    .collect::<Vec<_>>()
            ));
        }
        if action == "models.list" {
            return Ok(json!(Kind::ALL.iter().map(|kind| {
                let account = crate::accounts::get(cx)
                    .map(|center| center.read(cx).selected(*kind, &self.workspace.project))
                    .unwrap_or_default();
                json!({"harness":kind.id(),"models":vorkspace_core::catalog::cached_models_for(*kind,&account).unwrap_or_default()})
            }).collect::<Vec<_>>()));
        }
        let project = action
            .starts_with("sessions.")
            .then(|| {
                voice_project(
                    &projects,
                    input["project"].as_str().unwrap_or(&self.workspace.project),
                )
            })
            .transpose()?;
        if action == "sessions.list" {
            return Ok(json!(self.order.iter().filter_map(|id|{let s=self.session_metadata(*id,cx);(Some(s.summary.cwd.as_str()) == project).then(||json!({"id":id,"title":s.summary.title,"cwd":s.summary.cwd,"busy":s.busy,"hasDraft":s.has_saved_draft(),"harness":s.summary.provider}))}).take(200).collect::<Vec<_>>()));
        }
        if action == "sessions.start" {
            let cwd = project.ok_or("Choose a known local project")?;
            let prompt = voice_text(input, "prompt")?;
            let kind = input["harness"]
                .as_str()
                .map(Kind::parse)
                .unwrap_or(Some(self.default_kind()))
                .ok_or("Unknown provider")?;
            let settings = vorkspace_core::agent::Settings {
                model: input["model"].as_str().map(str::to_owned).or_else(|| {
                    self.workspace
                        .preferences
                        .default_models
                        .get(kind.id())
                        .cloned()
                }),
                access: vorkspace_core::agent::AccessMode::from_id(
                    self.workspace
                        .preferences
                        .default_access
                        .as_deref()
                        .unwrap_or("supervised"),
                ),
                account: crate::accounts::get(cx)
                    .map(|center| center.read(cx).selected(kind, cwd))
                    .unwrap_or_default(),
                ..Default::default()
            };
            if let Some(model) = input["model"].as_str() {
                let catalog = vorkspace_core::catalog::cached_models_for(kind, &settings.account)
                    .ok_or(
                    "Open this provider's model picker first, then retry with a listed model",
                )?;
                if !catalog.iter().any(|m| m.id == model) {
                    return Err("Choose a model from the current provider account's catalog".into());
                }
            }
            let summary = {
                let db = locked(&self.store)?;
                let s = db.create(kind.id(), cwd)?;
                db.configure_agent(s.id, kind.id(), &settings)?;
                s
            };
            let id = summary.id;
            self.bot_install(summary, settings, prompt, false, cx);
            if input["reveal"] == true {
                self.open(id, window, cx);
            }
            return Ok(json!({"accepted":true,"sessionId":id}));
        }
        let bot_query = input["bot"].as_str().or_else(|| {
            (action == "bots.send")
                .then(|| input["to"].as_str())
                .flatten()
        });
        if bot_query.is_some() && input.get("sessionId").is_some() {
            return Err("Choose one Bot or session".into());
        }
        let mut bot = bot_query
            .map(|q| vorkspace_core::bot::channels::resolve(&bots, q).cloned())
            .transpose()?;
        if bot.is_none()
            && input.get("sessionId").is_none()
            && matches!(action, "composer.read" | "composer.write")
            && self.bot_active(cx)
        {
            bot = self
                .bots
                .as_ref()
                .and_then(|m| m.read(cx).active.as_deref())
                .and_then(|id| bots.get(id))
                .cloned();
        }
        if action == "workspace.open" && bot.is_none() && input.get("sessionId").is_none() {
            return Err("Choose a Bot or session to open".into());
        }
        if action == "bots.send" {
            let bot = bot.ok_or("Choose a Bot")?;
            let message = voice_text(input, "message")?;
            self.bots
                .as_ref()
                .ok_or("Bots unavailable")?
                .update(cx, |m, cx| m.send_message(&bot.id, message, Vec::new(), cx))?;
            return Ok(json!({"accepted":true,"botId":bot.id}));
        }
        let view = if let Some(bot) = &bot {
            self.bots
                .as_ref()
                .ok_or("Bots unavailable")?
                .update(cx, |m, cx| m.chat_for_action(&bot.id, cx))
                .ok_or("Bot conversation is opening; try again")?
        } else {
            let id = input["sessionId"]
                .as_i64()
                .or_else(|| input["sessionId"].as_str()?.parse().ok())
                .or_else(|| {
                    self.workspace
                        .active
                        .map(|id| self.workspace.session_for_pane(id))
                })
                .ok_or("Choose a session")?;
            if !self
                .session_summaries
                .get(&id)
                .is_some_and(|s| projects.contains(&s.cwd))
            {
                return Err("Session is outside known local projects".into());
            }
            self.ensure_session(id, cx).ok_or("Session unavailable")?
        };
        if project.is_some_and(|p| view.read(cx).summary.cwd != p) {
            return Err("Session does not belong to the requested project".into());
        }
        let id = view.read(cx).summary.id;
        match action {
            "workspace.open" => {
                if let Some(bot) = bot {
                    self.click_bot_tab(&bot.id, window, cx);
                } else {
                    self.open(id, window, cx);
                }
                Ok(json!({"opened":true,"sessionId":id}))
            }
            "sessions.stop" => {
                view.update(cx, |v, cx| v.stop_if_busy(cx));
                Ok(json!({"accepted":true}))
            }
            "sessions.send" => {
                let text = voice_text(input, "prompt")?;
                view.update(cx, |v, cx| v.voice_send(text, cx))?;
                Ok(json!({"accepted":true}))
            }
            "composer.read" | "composer.write" => {
                if !view.read(cx).voice_composer_ready() {
                    view.update(cx, |v, cx| {
                        if v.needs_history_restore() {
                            v.load(cx);
                        }
                    });
                    return Err("The saved composer is still loading; try again".into());
                }
                let input_entity = view.read(cx).input.clone();
                if action == "composer.read" {
                    let text = &input_entity.read(cx).content;
                    return Ok(
                        json!({"text":text.chars().take(6000).collect::<String>(),"truncated":text.chars().count()>6000,"selection":{"start":input_entity.read(cx).selection_utf16().start,"end":input_entity.read(cx).selection_utf16().end}}),
                    );
                }
                let text = voice_text(input, "text")?;
                input_entity.update(cx, |field, cx| {
                    if field.is_composing() {
                        return Err("Finish typing before Vox edits this composer".to_string());
                    }
                    let value = voice_draft(
                        &field.content,
                        &text,
                        input["mode"].as_str().unwrap_or("append"),
                        input["expectedText"].as_str(),
                    )?;
                    field.set_text(value, cx);
                    Ok(())
                })?;
                Ok(json!({"sent":false}))
            }
            "sessions.read" | "bots.read" => {
                if !view.read(cx).voice_composer_ready() {
                    view.update(cx, |v, cx| {
                        if v.needs_history_restore() {
                            v.load(cx);
                        }
                    });
                    return Err("Saved conversation is loading; try again".into());
                }
                let rows = view.read(cx).bot_rows(cx);
                let end = input["before"]
                    .as_u64()
                    .unwrap_or(rows.len() as u64)
                    .min(rows.len() as u64) as usize;
                let limit = input["limit"].as_u64().unwrap_or(3).clamp(1, 3) as usize;
                let max = input["maxChars"].as_u64().unwrap_or(6000).clamp(200, 6000) as usize;
                let start = end.saturating_sub(limit * 2);
                Ok(
                    json!({"sessionId":id,"messages":rows[start..end].iter().filter(|r|r.role=="user"||r.role=="assistant").map(|r|json!({"role":r.role,"text":r.text.chars().take(max/(limit*2)).collect::<String>()})).collect::<Vec<_>>(),"nextBefore":(start>0).then_some(start)}),
                )
            }
            _ => Err("Unsupported Vox action".into()),
        }
    }
}
fn voice_text(input: &Value, key: &str) -> Result<String, String> {
    input[key]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 65_536)
        .map(str::to_owned)
        .ok_or_else(|| format!("Provide {key}, up to 65536 bytes"))
}
fn voice_project<'a>(projects: &'a [String], query: &str) -> Result<&'a str, String> {
    if let Some(exact) = projects.iter().find(|p| p.as_str() == query) {
        return Ok(exact);
    }
    let mut matches = projects
        .iter()
        .filter(|p| crate::workspace::project_name(p) == query);
    match (matches.next(), matches.next()) {
        (Some(project), None) => Ok(project),
        _ => Err("Choose one known local project; ambiguous names require a full path".into()),
    }
}
fn voice_draft(
    current: &str,
    text: &str,
    mode: &str,
    expected: Option<&str>,
) -> Result<String, String> {
    match mode {
        "replace" if expected == Some(current) => Ok(text.into()),
        "replace" => {
            Err("Composer changed since it was read; read it again before replacing".into())
        }
        "append" => Ok(if current.is_empty() {
            text.into()
        } else {
            format!("{current}\n{text}")
        }),
        _ => Err("Mode must be append or replace".into()),
    }
}
fn validate_voice_input(action: &str, input: &Value) -> Result<(), String> {
    let fields = match action {
        "workspace.current" | "bots.list" | "models.list" => "",
        "sessions.list" => "project",
        "sessions.start" => "prompt project harness model reveal notifyOnComplete",
        "sessions.send" => "sessionId prompt project",
        "sessions.stop" => "sessionId project",
        "bots.send" => "bot to message expectReply",
        "sessions.read" => "sessionId project before limit maxChars",
        "bots.read" => "bot before limit maxChars",
        "workspace.open" | "composer.read" => "sessionId bot",
        "composer.write" => "sessionId bot text mode expectedText",
        _ => return Err("Unsupported Vox action".into()),
    };
    for key in ["notifyOnComplete", "expectReply"] {
        if input.get(key).is_some_and(|value| value != false) {
            return Err(format!("{key} must be false for Vox"));
        }
    }
    for (key, value) in input.as_object().ok_or("Input must be an object")? {
        if !fields.split_whitespace().any(|f| f == key) {
            return Err(format!("{action} does not take {key}"));
        }
        let valid = match key.as_str() {
            "sessionId" => value.as_i64().is_some() || value.is_string(),
            "before" | "limit" | "maxChars" => value.as_u64().is_some(),
            "reveal" | "notifyOnComplete" | "expectReply" => value.is_boolean(),
            _ => value.is_string(),
        };
        if !valid {
            return Err(format!("Invalid {key}"));
        }
    }
    let required = match action {
        "sessions.read" | "sessions.stop" => "sessionId",
        "sessions.send" => "sessionId prompt",
        "sessions.start" => "prompt",
        "bots.read" => "bot",
        "bots.send" => "message",
        "composer.write" => "text",
        _ => "",
    };
    for key in required.split_whitespace() {
        if input.get(key).is_none_or(Value::is_null) {
            return Err(format!("{action} requires {key}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn moving_vox_to_its_own_window_preserves_workspace_action_ownership(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, _, _) = crate::shell::tests::window(cx);
        let (voice, owner) = workspace
            .update(cx, |shell, _, cx| {
                (shell.vox.clone().unwrap(), cx.weak_entity())
            })
            .unwrap();
        let panel = cx.update(|cx| {
            cx.open_window(Default::default(), |_, _| voice.clone())
                .unwrap()
        });
        cx.run_until_parked();
        let voice_window = voice
            .downgrade()
            .update_in(cx, |_, window, _| window.window_handle().window_id())
            .unwrap();
        let workspace_window = owner
            .update_in(cx, |_, window, _| window.window_handle().window_id())
            .unwrap();
        assert_eq!(voice_window, panel.window_id());
        assert_eq!(workspace_window, workspace.window_id());
        assert_ne!(voice_window, workspace_window);
    }

    #[gpui::test]
    fn vox_launcher_sits_above_subscriptions_and_disabled_vox_opens_settings(
        cx: &mut gpui::TestAppContext,
    ) {
        crate::clock::use_test_clock(cx);
        let (window, _, _) = crate::shell::tests::window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1400.), px(900.)));
        for expanded in [true, false] {
            window
                .update(visual, |shell, _, cx| {
                    shell.workspace.project_rail_open = expanded;
                    shell.workspace.sidebar_subscriptions_visible = true;
                    shell.workspace.preferences.vox_enabled = false;
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            let vox = visual.debug_bounds("vox-launcher").unwrap();
            let subscriptions = visual.debug_bounds("subscriptions").unwrap();
            assert!(vox.bottom() <= subscriptions.top(), "expanded={expanded}");
        }
        let button = visual.debug_bounds("vox-launcher").unwrap();
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert!(shell.settings.is_some());
                assert!(!shell.vox.as_ref().unwrap().read(cx).running());
                assert_eq!(
                    cx.windows().len(),
                    1,
                    "enabling Vox never starts microphone capture"
                );
            })
            .unwrap();
    }

    #[test]
    fn documented_voice_flags_are_validated_before_effects() {
        for (action, input) in [
            (
                "sessions.start",
                json!({"project":"/test","prompt":"hello","notifyOnComplete":false}),
            ),
            (
                "bots.send",
                json!({"to":"Helper","message":"hello","expectReply":false}),
            ),
            (
                "sessions.read",
                json!({"project":"/test","sessionId":1,"limit":3}),
            ),
        ] {
            assert!(validate_voice_input(action, &input).is_ok(), "{action}");
        }
        assert!(
            validate_voice_input(
                "sessions.start",
                &json!({"prompt":"hello","notifyOnComplete":true})
            )
            .is_err()
        );
        assert!(
            validate_voice_input(
                "sessions.start",
                &json!({"prompt":"hello","runtimeMode":"full-access"})
            )
            .is_err()
        );
    }
    #[test]
    fn deliberate_typing_wins_over_delayed_voice_replace() {
        assert!(voice_draft("new typing", "replacement", "replace", Some("old draft")).is_err());
        assert_eq!(
            voice_draft("typed", "voice", "append", None).unwrap(),
            "typed\nvoice"
        );
        assert_eq!(
            voice_draft("typed", "replacement", "replace", Some("typed")).unwrap(),
            "replacement"
        );
    }
    #[test]
    fn voice_targets_require_explicit_sessions_and_unambiguous_projects() {
        let projects = vec![
            "/work/app".into(),
            "/personal/app".into(),
            "/work/unique".into(),
        ];
        assert!(voice_project(&projects, "app").is_err());
        assert!(voice_project(&projects, "/unopened").is_err());
        assert_eq!(voice_project(&projects, "/work/app").unwrap(), "/work/app");
        assert_eq!(voice_project(&projects, "unique").unwrap(), "/work/unique");
        for action in [
            "sessions.read",
            "sessions.send",
            "sessions.stop",
            "bots.read",
        ] {
            assert!(
                validate_voice_input(action, &json!({})).is_err(),
                "{action}"
            );
        }
    }
}
