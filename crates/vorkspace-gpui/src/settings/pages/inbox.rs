use crate::input::{self, Input};
use crate::settings::{SettingsEvent, SettingsView, widgets::secondary_button};
use gpui::{AnyElement, Context, Entity, SharedString, div, prelude::*, px};
use vorkspace_core::inbox::JiraFilter;
use vorkspace_ui::theme::*;

#[derive(Default)]
pub(in crate::settings) struct InboxState {
    query: Option<Entity<Input>>,
    mode: String,
    pub(in crate::settings) revision: u64,
    applying: bool,
    pending: Option<u64>,
    error: Option<String>,
}

impl SettingsView {
    fn initialize_jira_query(&mut self, cx: &mut Context<Self>) {
        if self.inbox.query.is_some() {
            return;
        }
        self.inbox.mode = self.ctx.jira_filter.mode.clone();
        let query = self.ctx.jira_filter.jql.clone();
        let input = cx.new(|cx| {
            let mut input = Input::field(
                "JQL query",
                "project = ENG ORDER BY updated DESC",
                content(0.4),
                cx,
            );
            input.set_text(query, cx);
            input
        });
        self._subscriptions
            .push(cx.subscribe(&input, |this, _, _: &input::Changed, cx| {
                this.inbox.revision += 1;
                this.inbox.error = None;
                cx.notify();
            }));
        self._subscriptions
            .push(cx.subscribe(&input, |this, _, _: &input::Submitted, cx| {
                this.apply_jira_jql(cx)
            }));
        self.inbox.query = Some(input);
    }
    fn apply_jira_jql(&mut self, cx: &mut Context<Self>) {
        if self.inbox.applying
            || self.inbox.mode != "jql"
            || !self.ctx.inbox_connectors.enabled("jira")
        {
            return;
        }
        let query = self
            .inbox
            .query
            .as_ref()
            .map(|query| query.read(cx).content.trim().to_owned())
            .unwrap_or_default();
        if query.is_empty() {
            self.inbox.error = Some("Enter a JQL query.".into());
            cx.notify();
            return;
        }
        let Some(directory) = self.ctx.inbox_data_dir.clone() else {
            self.inbox.error = Some("Jira data directory is unavailable".into());
            cx.notify();
            return;
        };
        self.inbox.revision += 1;
        let revision = self.inbox.revision;
        let connection = self.ctx.inbox_connections.get("jira").cloned();
        self.inbox.applying = true;
        self.inbox.pending = Some(revision);
        self.inbox.error = None;
        let candidate = query.clone();
        let task = cx.background_executor().spawn(async move {
            vorkspace_core::inbox::jira::jira_list_issues(
                directory,
                false,
                "all".into(),
                vec![],
                Some(1),
                Some(candidate),
            )
            .map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_jira_jql(revision, query, connection, result, cx)
            });
        })
        .detach();
        cx.notify();
    }
    fn finish_jira_jql(
        &mut self,
        revision: u64,
        query: String,
        connection: Option<(bool, String)>,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if self.inbox.pending != Some(revision) {
            return;
        }
        self.inbox.pending = None;
        self.inbox.applying = false;
        if self.inbox.revision != revision {
            cx.notify();
            return;
        }
        if self.inbox.mode != "jql"
            || !self.ctx.inbox_connectors.enabled("jira")
            || self.ctx.inbox_connections.get("jira").cloned() != connection
            || self
                .inbox
                .query
                .as_ref()
                .is_none_or(|input| input.read(cx).content.trim() != query)
        {
            cx.notify();
            return;
        }
        match result {
            Ok(()) => cx.emit(SettingsEvent::JiraFilter(JiraFilter {
                mode: "jql".into(),
                jql: query,
            })),
            Err(error) => self.inbox.error = Some(error),
        }
        cx.notify();
    }
    fn jira_filter_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.initialize_jira_query(cx);
        let select = self.select(
            "jira-filter-mode",
            "Filter tickets by",
            &self.inbox.mode,
            vec![
                ("project".into(), "Project".into(), None),
                ("jql".into(), "JQL".into(), None),
            ],
            Some(180.),
            cx,
            |this, mode, cx| {
                this.inbox.revision += 1;
                this.inbox.error = None;
                this.inbox.mode = mode.clone();
                if mode == "project" {
                    let mut filter = this.ctx.jira_filter.clone();
                    filter.mode = mode;
                    cx.emit(SettingsEvent::JiraFilter(filter));
                }
                cx.notify();
            },
        );
        let mut rows = vec![self.row(
            Some("jira-filter-mode"),
            false,
            "Filter tickets by",
            None,
            vec![select],
        )];
        if self.inbox.mode == "jql" {
            let query = self.inbox.query.clone().unwrap();
            let applied = self.ctx.jira_filter.mode == "jql"
                && self.ctx.jira_filter.jql == query.read(cx).content.trim();
            let apply = secondary_button(
                "jira-apply-jql",
                "Apply JQL",
                self.inbox.applying || applied || query.read(cx).content.trim().is_empty(),
                false,
            )
            .debug_selector(|| "settings-jira-apply-jql".into())
            .child(if self.inbox.applying {
                "Checking JQL…"
            } else {
                "Apply JQL"
            })
            .on_click(cx.listener(|this, _, _, cx| this.apply_jira_jql(cx)))
            .into_any_element();
            rows.push(div().id("jira-query-row").debug_selector(||"settings-jira-query".into()).px(px(20.)).py(px(12.)).flex().flex_col().gap(px(10.)).text_size(px(12.)).child("JQL query")
                .child(div().h(px(36.)).px_2().border_1().border_color(content(0.15)).rounded_md().flex().items_center().child(query))
                .child(div().text_color(content(0.5)).child("JQL controls which tickets are fetched. Project selections and Assigned to me do not apply. Jira status, time, and search filters still narrow the results."))
                .child(apply).child(div().text_color(content(0.5)).child(if applied {"JQL query applied."}else{"Apply to check and save your query."}))
                .when_some(self.inbox.error.clone(),|d,error|d.child(div().text_color(gpui::rgb(0xef4444)).child(error))).into_any_element());
        } else {
            let refresh =
                secondary_button("jira-refresh-projects", "Refresh projects", false, false)
                    .child("Refresh projects")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::RefreshInbox)))
                    .into_any_element();
            rows.push(self.row(
                None,
                false,
                "Projects",
                Some("Selections are shared with the Jira filter menu.".into()),
                vec![refresh],
            ));
            for (id, name) in self.ctx.jira_projects.clone() {
                let visible = !self.ctx.hidden_jira_projects.contains(&id);
                let control = secondary_button(
                    SharedString::from(format!("jira-project-{id}")),
                    "Toggle Jira project",
                    false,
                    false,
                )
                .child(icon(if visible { "check" } else { "minus" }, 14.))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(SettingsEvent::JiraProject(id.clone(), !visible))
                }))
                .into_any_element();
                rows.push(self.row(None, false, name, None, vec![control]));
            }
        }
        rows
    }
    fn inbox_bot_row(&self, source: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let id = if source == "jira" {
            "jira-default-bot"
        } else {
            "gitlab-default-bot"
        };
        let selected = self.ctx.inbox_bots.get(source).unwrap_or_default();
        let mut options = vec![(String::new(), "No default Bot".into(), None)];
        options.extend(
            self.ctx
                .bots
                .iter()
                .map(|bot| (bot.id.clone(), bot.name.clone().into(), None)),
        );
        if !selected.is_empty() && !self.ctx.bots.iter().any(|bot| bot.id == selected) {
            options.push((selected.into(), "Unavailable Bot".into(), None));
        }
        let select = self.select(
            id,
            "Default Bot",
            selected,
            options,
            Some(208.),
            cx,
            move |_, bot, cx| cx.emit(SettingsEvent::InboxBot(source.into(), bot)),
        );
        self.row(Some(id),false,"Default Bot",Some("Preselected when you send items to chat. You can choose another destination before sending.".into()),vec![select])
    }
    pub(super) fn inbox_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut page = vec![self.project_notification_settings(cx)];
        for (id, title, description, label, hint) in [
            (
                "github",
                "GitHub",
                "Pull requests, reviews, and issues, read through the GitHub CLI.",
                "Connection",
                "Install GitHub CLI from cli.github.com, run gh auth login in a terminal, then check again.",
            ),
            (
                "gitlab",
                "GitLab",
                "Merge requests from GitLab.com or a self-managed instance.",
                "Connection",
                "Connect GitLab.com or a self-managed GitLab instance. Use a personal access token with API access; the token is stored locally and Disconnect deletes it.",
            ),
            (
                "jira",
                "Jira",
                "Jira Cloud issues from the projects you pick.",
                "Connection",
                "Connect your Jira Cloud site with your email and an API token.",
            ),
        ] {
            let enabled = self.ctx.inbox_connectors.enabled(id);
            let (connected, connection_label) = self
                .ctx
                .inbox_connections
                .get(id)
                .cloned()
                .unwrap_or_else(|| (false, "Checking connection…".into()));
            let status = div()
                .text_size(px(12.))
                .text_color(content(if connected { 0.7 } else { 0.5 }))
                .child(connection_label)
                .into_any_element();
            let controls = if id == "github" {
                vec![
                    status,
                    secondary_button(
                        "github-installation-guide",
                        "Installation guide",
                        false,
                        false,
                    )
                    .child("Installation guide")
                    .on_click(|_, _, cx| cx.open_url("https://cli.github.com/"))
                    .into_any_element(),
                    secondary_button("github-check-again", "Check again", false, false)
                        .child("Check again")
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(SettingsEvent::ManageInboxProvider("github".into()))
                        }))
                        .into_any_element(),
                ]
            } else {
                vec![
                    status,
                    secondary_button(
                        SharedString::from(format!("connect-{id}")),
                        if connected { "Manage" } else { "Connect" },
                        false,
                        false,
                    )
                    .child(if connected { "Manage" } else { "Connect" })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SettingsEvent::ManageInboxProvider(id.into()))
                    }))
                    .into_any_element(),
                ]
            };
            let row = if enabled {
                self.row(None, false, label, Some(hint.into()), controls)
            } else {
                self.row(
                    None,
                    false,
                    "Connector disabled",
                    Some("Saved connection details are kept.".into()),
                    vec![],
                )
            };
            let toggle_id = match id {
                "github" => "inbox-connector-github",
                "gitlab" => "inbox-connector-gitlab",
                _ => "inbox-connector-jira",
            };
            let enable = self.toggle(
                toggle_id,
                format!("Enable {title} connector"),
                enabled,
                false,
                cx,
                move |_, on, cx| {
                    cx.emit(SettingsEvent::InboxConnectorEnabled(id.into(), on));
                },
            );
            let mut rows = vec![row];
            if enabled {
                if matches!(id, "jira" | "gitlab") {
                    rows.push(self.inbox_bot_row(id, cx));
                }
                if id == "jira" && connected {
                    rows.extend(self.jira_filter_rows(cx));
                }
            }
            page.push(
                self.group(
                    Some(id),
                    false,
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon(
                            match id {
                                "jira" => "inbox-jira",
                                "gitlab" => "inbox-gitlab",
                                _ => "inbox-github",
                            },
                            16.,
                        ))
                        .child(title),
                    Some(description.into()),
                    Some(enable),
                    rows,
                ),
            );
        }
        page
    }
}

#[cfg(test)]
mod tests;
