//! Inbox filters, connection and action menus.
use super::*;

impl Inbox {
    pub(super) fn filters_active(&self) -> bool {
        let f = &self.state.filters;
        (f.assigned_to_me && !(self.state.source == "jira" && self.state.jira_filter.mode == "jql"))
            || !f.hidden_projects.is_empty()
            || !f.hidden_kinds.is_empty()
            || f.time != "all"
            || f.status.open
            || f.status.draft
            || f.status.closed
            || f.status.merged
            || (self.state.source == "jira"
                && self.state.jira_filter.mode != "jql"
                && !self.state.hidden_jira_projects.is_empty())
    }
    pub(crate) fn filters_changed(&mut self, refetch: bool, cx: &mut Context<Self>) {
        if self.state.source == "jira"
            && let Some(owner) = self.owner.clone()
        {
            let hidden = self.state.hidden_jira_projects.clone();
            let _ = owner.update(cx, |owner, _| owner.state.hidden_jira_projects = hidden);
        }
        self.save(cx);
        if self.active
            && let Some(item) = self.visible_items(cx).first()
        {
            self.select(item.key(), cx);
        }
        if refetch {
            self.refresh(cx);
        }
        cx.notify();
    }
    pub(super) fn filters_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let f = &self.state.filters;
        let tracker = model::is_tracker(&self.state.source);
        let mut rows = vec![caption("FILTER ITEMS").into_any_element()];
        if !(tracker && self.state.jira_filter.mode == "jql") {
            rows.push(
                menu_row(
                    "inbox-assigned",
                    if self.state.source == "gitlab" {
                        "Needs my attention"
                    } else {
                        "Assigned to me"
                    },
                    f.assigned_to_me,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.filters.assigned_to_me = !this.state.filters.assigned_to_me;
                    this.filters_changed(true, cx);
                }))
                .into_any_element(),
            );
        }
        rows.push(caption("UPDATED").into_any_element());
        for (time, label) in [
            ("all", "Any time"),
            ("today", "Today"),
            ("7d", "Last 7 days"),
            ("30d", "Last 30 days"),
        ] {
            rows.push(
                menu_row(format!("inbox-time-{time}"), label, f.time == time)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.filters.time = time.into();
                        this.filters_changed(false, cx);
                    }))
                    .into_any_element(),
            );
        }
        rows.push(caption("STATUS").into_any_element());
        for status in ["Open", "Draft", "Closed", "Merged"]
            .into_iter()
            .filter(|s| !tracker || matches!(*s, "Open" | "Closed"))
        {
            let checked = match status {
                "Open" => f.status.open,
                "Draft" => f.status.draft,
                "Closed" => f.status.closed,
                _ => f.status.merged,
            };
            rows.push(
                menu_row(format!("inbox-status-{status}"), status, checked)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let s = &mut this.state.filters.status;
                        match status {
                            "Open" => s.open = !s.open,
                            "Draft" => s.draft = !s.draft,
                            "Closed" => s.closed = !s.closed,
                            _ => s.merged = !s.merged,
                        }
                        this.filters_changed(true, cx);
                    }))
                    .into_any_element(),
            );
        }
        if !tracker {
            rows.push(caption("TYPE").into_any_element());
            for (kind, label) in [
                ("issue", "Issues"),
                (
                    "pr",
                    if self.state.source == "gitlab" {
                        "Merge requests"
                    } else {
                        "Pull requests"
                    },
                ),
            ] {
                rows.push(
                    menu_row(
                        format!("inbox-kind-{kind}"),
                        label,
                        !f.hidden_kinds.iter().any(|k| k == kind),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle(&mut this.state.filters.hidden_kinds, kind.into());
                        this.filters_changed(false, cx);
                    }))
                    .into_any_element(),
                );
            }
            rows.push(caption("PROJECTS").into_any_element());
            for (ix, project) in self.projects.iter().enumerate() {
                let key = project.to_string_lossy().into_owned();
                rows.push(
                    menu_row(
                        format!("inbox-project-filter-{ix}"),
                        project_name(project),
                        !f.hidden_projects.contains(&key),
                    )
                    .child(self.project_mark(&key, 12.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle(&mut this.state.filters.hidden_projects, key.clone());
                        this.filters_changed(false, cx);
                    }))
                    .into_any_element(),
                );
            }
        }
        let provider_options = &self.snapshot.jira_projects;
        if tracker && self.state.jira_filter.mode != "jql" && !provider_options.is_empty() {
            rows.push(caption("PROJECTS").into_any_element());
            for (ix, option) in provider_options.iter().enumerate() {
                let id = option.id.clone();
                let hidden = &self.state.hidden_jira_projects;
                rows.push(
                    menu_row(
                        format!("inbox-tracker-filter-{ix}"),
                        option.name.clone(),
                        !hidden.contains(&id),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle(&mut this.state.hidden_jira_projects, id.clone());
                        this.filters_changed(true, cx);
                    }))
                    .into_any_element(),
                );
            }
        }
        rows.push(
            action("inbox-clear-filters", "Clear filters")
                .w_full()
                .justify_start()
                .mt(px(8.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.filters = model::Filters::default();
                    this.state.hidden_jira_projects.clear();
                    this.filters_changed(true, cx);
                }))
                .into_any_element(),
        );
        popover(
            "inbox-filter-menu",
            Anchor::Slot(self.filter_anchor.clone()),
        )
        .align(Align::End)
        .width(252.)
        .max_height(620.)
        .padding(4.)
        .on_dismiss(move |_, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.filters_open = false;
                cx.notify();
            });
        })
        .child(
            div()
                .id("inbox-filter-scroll")
                .overflow_y_scroll()
                .children(rows),
        )
        .into_any_element()
    }
    pub(super) fn connection_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        popover(
            "inbox-connections-menu",
            Anchor::Slot(self.connection_anchor.clone()),
        )
        .width(272.)
        .padding(4.)
        .on_dismiss(move |_, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.connections_open = false;
                cx.notify();
            });
        })
        .child(caption("CONNECTIONS"))
        .children(self.state.connectors.enabled_providers().map(|provider| {
            let connected = self
                .snapshot
                .connections
                .get(provider)
                .is_some_and(|s| s.connected);
            let label = format!(
                "{} {}",
                if connected { "Manage" } else { "Connect" },
                model::provider_label(provider)
            );
            div()
                .id(SharedString::from(format!("inbox-connect-{provider}")))
                .debug_selector(move || format!("inbox-connect-{provider}"))
                .role(Role::Button)
                .aria_label(label.clone())
                .w_full()
                .h(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(content(0.7))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)).text_color(content(1.)))
                .child(provider_mark(provider))
                .child(label)
                .on_click(
                    cx.listener(move |this, _, _, cx| this.open_connection(provider.into(), cx)),
                )
        }))
        .into_any_element()
    }

    pub(super) fn connection_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut background = shell_fill();
        background.a = 0.9;
        let provider = self.connect_provider.clone().unwrap_or_default();
        let connected = self
            .snapshot
            .connections
            .get(&provider)
            .is_some_and(|s| s.connected);
        let title = format!(
            "{} {}",
            if connected { "Manage" } else { "Connect" },
            model::provider_label(&provider)
        );
        let hint = match provider.as_str() {
            "github" => {
                "Install GitHub CLI, then run gh auth login in a terminal. Check again when signed in."
            }
            "jira" => "Connect your Jira Cloud site with your Atlassian email and an API token.",
            "gitlab" => {
                "Connect GitLab.com or your GitLab instance with a personal access token with API access."
            }
            _ => "Unsupported connector.",
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000040))
            .child(
                div()
                    .w(px(440.))
                    .max_w_full()
                    .p(px(24.))
                    .rounded(px(16.))
                    .border_1()
                    .border_color(content(0.1))
                    .bg(background)
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(provider_mark(&provider))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(content(1.))
                                    .child(title),
                            )
                            .child(
                                icon_button("inbox-close-connect", "Close connection", "close")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.connecting {
                                            this.connect_provider = None;
                                            this.connect_token.update(cx, |input, cx| {
                                                input.set_text(String::new(), cx)
                                            });
                                            cx.notify();
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(19.))
                            .text_color(content(0.5))
                            .child(hint),
                    )
                    .when(provider != "github", |d| {
                        d.child(
                            div()
                                .px(px(10.))
                                .py(px(8.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(content(0.1))
                                .child(self.connect_url.clone()),
                        )
                    })
                    .when(provider == "jira", |d| {
                        d.child(
                            div()
                                .px(px(10.))
                                .py(px(8.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(content(0.1))
                                .child(self.connect_email.clone()),
                        )
                    })
                    .when(provider != "github", |d| {
                        d.child(
                            div()
                                .px(px(10.))
                                .py(px(8.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(content(0.1))
                                .child(self.connect_token.clone()),
                        )
                    })
                    .when_some(self.connect_error.clone(), |d, text| d.child(error(text)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .when(connected && provider != "github", |d| {
                                d.child(
                                    action("inbox-disconnect", "Disconnect")
                                        .text_color(rgb(0xf87171))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.connect(true, cx)),
                                        ),
                                )
                            })
                            .when(provider == "github", |d| {
                                d.child(
                                    action("inbox-gh-guide", "Installation guide").on_click(
                                        |_, _, cx| cx.open_url("https://cli.github.com/"),
                                    ),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                action(
                                    "inbox-connect-submit",
                                    if self.connecting {
                                        "Connecting…"
                                    } else if provider == "github" {
                                        "Check again"
                                    } else {
                                        "Connect"
                                    },
                                )
                                .bg(content(1.))
                                .text_color(floating_fill())
                                .when(self.connecting, |d| d.opacity(0.4))
                                .on_click(cx.listener(|this, _, _, cx| this.connect(false, cx))),
                            ),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn project_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        popover(
            "inbox-project-menu",
            Anchor::Slot(self.project_anchor.clone()),
        )
        .width(240.)
        .padding(4.)
        .on_dismiss(move |_, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.project_picker = false;
                cx.notify();
            });
        })
        .children(self.projects.iter().enumerate().map(|(ix, project)| {
            let path = project.clone();
            menu_row(
                format!("inbox-start-project-{ix}"),
                project_name(project),
                self.start_bot.is_none() && *project == self.start_project,
            )
            .child(self.project_mark(&project.to_string_lossy(), 14.))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.start_project = path.clone();
                this.start_bot = None;
                this.destination_explicit = true;
                this.project_picker = false;
                cx.notify();
            }))
        }))
        .children(
            self.bots
                .iter()
                .filter(|_| {
                    self.item()
                        .is_some_and(|item| matches!(item.provider.as_str(), "jira" | "gitlab"))
                })
                .map(|(id, name)| {
                    let id = id.clone();
                    menu_row(
                        format!("inbox-start-bot-{id}"),
                        name.clone(),
                        self.start_bot.as_ref() == Some(&id),
                    )
                    .child(icon("bot", 14.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start_bot = Some(id.clone());
                        this.destination_explicit = true;
                        this.project_picker = false;
                        cx.notify();
                    }))
                }),
        )
        .into_any_element()
    }

    pub(super) fn action_menu_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        popover(
            "inbox-action-menu",
            Anchor::Slot(self.action_anchor.clone()),
        )
        .width(272.)
        .padding(4.)
        .on_dismiss(move |_, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.action_menu = false;
                cx.notify();
            });
        })
        .children(
            [
                (
                    "merge",
                    "Create a merge commit",
                    "Add every commit to the base branch.",
                ),
                (
                    "squash",
                    "Squash and merge",
                    "Combine the commits into one.",
                ),
                (
                    "rebase",
                    "Rebase and merge",
                    "Add the commits without a merge commit.",
                ),
            ]
            .into_iter()
            .map(|(name, label, description)| {
                div()
                    .id(format!("inbox-merge-{name}"))
                    .role(Role::MenuItemRadio)
                    .aria_label(label)
                    .w_full()
                    .px(px(8.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.08)))
                    .child(div().mt(px(4.)).size(px(6.)).rounded_full().bg(
                        if self.merge_action == name {
                            accent_color()
                        } else {
                            content(0.2)
                        },
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(content(0.9))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(11.))
                                    .line_height(px(15.))
                                    .text_color(content(0.45))
                                    .child(description),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.merge_action = name.into();
                        this.action_menu = false;
                        cx.notify();
                    }))
            }),
        )
        .into_any_element()
    }
    pub(super) fn action_confirmation(&self, cx: &mut Context<Self>) -> AnyElement {
        let action_name = self.confirm_action.clone().unwrap_or_default();
        let details = self.details.clone().unwrap_or_default();
        let source = if details.head_ref_name.is_empty() {
            "this branch".into()
        } else {
            format!("“{}”", details.head_ref_name)
        };
        let destination = if details.base_ref_name.is_empty() {
            "the base branch".into()
        } else {
            format!("“{}”", details.base_ref_name)
        };
        let (title, detail, label, progress) = match action_name.as_str() {
            "merge" => (
                "Merge this pull request?",
                format!(
                    "Every commit from {source} will be added to {destination} with a merge commit."
                ),
                "Merge pull request",
                "Merging…",
            ),
            "squash" => (
                "Squash and merge?",
                format!(
                    "The commits from {source} will be combined into one commit on {destination}."
                ),
                "Squash and merge",
                "Merging…",
            ),
            "rebase" => (
                "Rebase and merge?",
                format!(
                    "The commits from {source} will be rebased individually onto {destination}."
                ),
                "Rebase and merge",
                "Merging…",
            ),
            "close" => (
                "Close this pull request?",
                "The pull request will close without merging. You can reopen it later.".into(),
                "Close pull request",
                "Closing…",
            ),
            "reopen" => (
                "Reopen this pull request?",
                "The pull request will return to the open state.".into(),
                "Reopen pull request",
                "Reopening…",
            ),
            "ready" => (
                "Mark as ready for review?",
                "Reviewers will see that this pull request is ready for feedback.".into(),
                "Ready for review",
                "Updating…",
            ),
            _ => (
                "Convert to draft?",
                "Reviewers will see that this pull request is not ready to merge.".into(),
                "Convert to draft",
                "Converting…",
            ),
        };
        let anchor = match action_name.as_str() {
            "ready" => self.pr_action_anchors[0].clone(),
            "draft" => self.pr_action_anchors[1].clone(),
            "close" => self.pr_action_anchors[2].clone(),
            "reopen" => self.pr_action_anchors[3].clone(),
            _ => self.action_anchor.clone(),
        };
        let weak = cx.entity().downgrade();
        popover("inbox-pr-confirmation", Anchor::Slot(anchor))
            .width(320.)
            .gap(5.)
            .padding(12.)
            .on_dismiss(move |_, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if !this.acting {
                        this.confirm_action = None;
                        this.action_error = None;
                        cx.notify();
                    }
                });
            })
            .child(
                div()
                    .id("inbox-pr-confirmation-dialog")
                    .role(Role::Dialog)
                    .aria_label(title)
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(1.))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(content(0.55))
                            .child(detail),
                    ),
            )
            .when_some(self.action_error.clone(), |d, text| {
                d.child(
                    div()
                        .mt(px(8.))
                        .text_size(px(11.))
                        .text_color(rgb(0xf87171))
                        .child(text),
                )
            })
            .child(
                div()
                    .mt(px(12.))
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        action("inbox-action-cancel", "Cancel")
                            .when(self.acting, |d| d.opacity(0.4))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.acting {
                                    this.confirm_action = None;
                                    this.action_error = None;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        action(
                            "inbox-action-confirm",
                            if self.acting { progress } else { label },
                        )
                        .font_weight(FontWeight::MEDIUM)
                        .bg(if action_name == "close" {
                            Hsla::from(rgb(0xf43f5e)).opacity(0.2)
                        } else if matches!(action_name.as_str(), "merge" | "squash" | "rebase") {
                            Hsla::from(rgb(0x10b981)).opacity(0.2)
                        } else {
                            content(1.)
                        })
                        .text_color(if action_name == "close" {
                            rgb(0xfda4af).into()
                        } else if matches!(action_name.as_str(), "merge" | "squash" | "rebase") {
                            rgb(0x6ee7b7).into()
                        } else {
                            shell_fill()
                        })
                        .when(self.acting, |d| d.opacity(0.6))
                        .on_click(cx.listener(|this, _, _, cx| this.run_action(cx))),
                    ),
            )
            .into_any_element()
    }
}
