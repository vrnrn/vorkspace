//! Inbox filters, connection and action menus.
use super::*;
use vorkspace_ui::radius;

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
    pub(crate) fn filters_changed(&mut self, cx: &mut Context<Self>) {
        self.save(cx);
        if self.active
            && let Some(key) = self.first_visible_key(cx)
        {
            self.select(key, cx);
        }
        cx.notify();
    }
    pub(super) fn filters_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let f = &self.state.filters;
        let tracker = model::is_tracker(&self.state.source);
        let mut rows = vec![caption("Filter items").into_any_element()];
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
                    this.filters_changed(cx);
                }))
                .into_any_element(),
            );
        }
        rows.push(caption("Updated").into_any_element());
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
                        this.filters_changed(cx);
                    }))
                    .into_any_element(),
            );
        }
        rows.push(caption("Status").into_any_element());
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
                        this.filters_changed(cx);
                    }))
                    .into_any_element(),
            );
        }
        if !tracker {
            rows.push(caption("Type").into_any_element());
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
                        this.filters_changed(cx);
                    }))
                    .into_any_element(),
                );
            }
            rows.push(caption("Projects").into_any_element());
            for (ix, project) in self.projects.iter().enumerate() {
                let key = project.to_string_lossy().into_owned();
                rows.push(
                    menu_row(
                        format!("inbox-project-filter-{ix}"),
                        project_name(project),
                        !f.hidden_projects.contains(&key),
                    )
                    .child(crate::project_mark::project_icon(&key, 12.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle(&mut this.state.filters.hidden_projects, key.clone());
                        this.filters_changed(cx);
                    }))
                    .into_any_element(),
                );
            }
        }
        let provider_options = &self.snapshot.jira_projects;
        if tracker && self.state.jira_filter.mode != "jql" && !provider_options.is_empty() {
            rows.push(caption("Projects").into_any_element());
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
                        this.filters_changed(cx);
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
                    this.filters_changed(cx);
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
    pub(super) fn connection_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut background = pane_fill();
        background.a = 0.9;
        let provider = self.connect.provider.clone().unwrap_or_default();
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
                "Install GitHub CLI, then run “gh auth login” in a terminal. Check again when signed in."
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
            .p(px(16.))
            .bg(gpui::rgba(0x00000040))
            .child(
                div()
                    .w(px(440.))
                    .max_w_full()
                    .p(px(24.))
                    .rounded(px(radius::MODAL))
                    .border_1()
                    .border_color(page::divider())
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
                            .child(page::title().flex_1().min_w_0().child(title))
                            .child(
                                icon_button("inbox-close-connect", "Close connection", "close")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.dismiss_connection(cx)),
                                    ),
                            ),
                    )
                    .child(page::body().child(hint))
                    .when(provider != "github", |d| {
                        d.child(
                            page::field_frame()
                                .border_1()
                                .border_color(page::divider())
                                .child(self.connect.url.clone()),
                        )
                    })
                    .when(provider == "jira", |d| {
                        d.child(
                            page::field_frame()
                                .border_1()
                                .border_color(page::divider())
                                .child(self.connect.email.clone()),
                        )
                    })
                    .when(provider != "github", |d| {
                        d.child(
                            page::field_frame()
                                .border_1()
                                .border_color(page::divider())
                                .child(self.connect.token.clone()),
                        )
                    })
                    .when_some(self.connect.error.clone(), |d, text| d.child(error(text)))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .when(connected && provider != "github", |d| {
                                d.child(
                                    page::secondary(
                                        "inbox-disconnect",
                                        "Disconnect",
                                        true,
                                        self.connect.connecting,
                                    )
                                    .child("Disconnect")
                                    .on_click(cx.listener(|this, _, _, cx| this.connect(true, cx))),
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
                            .child({
                                let label = if self.connect.connecting {
                                    "Connecting…"
                                } else if provider == "github" {
                                    "Check again"
                                } else {
                                    "Connect"
                                };
                                page::primary(
                                    "inbox-connect-submit",
                                    label,
                                    self.connect.connecting,
                                )
                                .child(label)
                                .on_click(cx.listener(|this, _, _, cx| this.connect(false, cx)))
                            }),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn project_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        popover(
            "inbox-project-menu",
            Anchor::Slot(self.start.project_anchor.clone()),
        )
        .width(240.)
        .padding(4.)
        .on_dismiss(move |_, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.start.project_picker = false;
                cx.notify();
            });
        })
        .children(self.projects.iter().enumerate().map(|(ix, project)| {
            let path = project.clone();
            menu_row(
                format!("inbox-start-project-{ix}"),
                project_name(project),
                self.start.bot.is_none() && *project == self.start.project,
            )
            .child(crate::project_mark::project_icon(
                &project.to_string_lossy(),
                14.,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.start.project = path.clone();
                this.start.bot = None;
                this.start.destination_explicit = true;
                this.start.project_picker = false;
                cx.notify();
            }))
        }))
        .children(
            self.start
                .bots
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
                        self.start.bot.as_ref() == Some(&id),
                    )
                    .child(icon("bot", 14.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start.bot = Some(id.clone());
                        this.start.destination_explicit = true;
                        this.start.project_picker = false;
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
            Anchor::Slot(self.actions.anchor.clone()),
        )
        .width(272.)
        .padding(4.)
        .on_dismiss(move |_, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.actions.menu = false;
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
                page::row(
                    format!("inbox-merge-{name}"),
                    label,
                    self.actions.merge_action == name,
                )
                .role(Role::MenuItemRadio)
                .aria_toggled(if self.actions.merge_action == name {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .w_full()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(div().mt(px(4.)).size(px(6.)).rounded_full().bg(
                    if self.actions.merge_action == name {
                        accent_color()
                    } else {
                        content(0.2)
                    },
                ))
                .child(
                    div()
                        .min_w_0()
                        .child(page::label().font_weight(FontWeight::MEDIUM).child(label))
                        .child(page::meta().child(description)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.actions.merge_action = name.into();
                    this.actions.menu = false;
                    cx.notify();
                }))
            }),
        )
        .into_any_element()
    }
    pub(super) fn action_confirmation(&self, cx: &mut Context<Self>) -> AnyElement {
        let action_name = self.actions.confirm_action.clone().unwrap_or_default();
        let details = self.detail.details.clone().unwrap_or_default();
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
            "ready" => self.actions.pr_action_anchors[0].clone(),
            "draft" => self.actions.pr_action_anchors[1].clone(),
            "close" => self.actions.pr_action_anchors[2].clone(),
            "reopen" => self.actions.pr_action_anchors[3].clone(),
            _ => self.actions.anchor.clone(),
        };
        let weak = cx.entity().downgrade();
        popover("inbox-pr-confirmation", Anchor::Slot(anchor))
            .width(320.)
            .gap(5.)
            .padding(12.)
            .on_dismiss(move |_, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if !this.actions.acting {
                        this.actions.confirm_action = None;
                        this.actions.error = None;
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
                    .child(page::label().font_weight(FontWeight::MEDIUM).child(title))
                    .child(page::meta().child(detail)),
            )
            .when_some(self.actions.error.clone(), |d, text| {
                d.child(
                    page::meta()
                        .mt(px(8.))
                        .text_color(status(StatusColor::Danger, 1.))
                        .child(text),
                )
            })
            .child(
                div()
                    .mt(px(12.))
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        page::secondary(
                            "inbox-action-cancel",
                            "Cancel",
                            false,
                            self.actions.acting,
                        )
                        .child("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.actions.acting {
                                this.actions.confirm_action = None;
                                this.actions.error = None;
                                cx.notify();
                            }
                        })),
                    )
                    .child(
                        action_enabled(
                            "inbox-action-confirm",
                            if self.actions.acting { progress } else { label },
                            !self.actions.acting,
                        )
                        .font_weight(FontWeight::MEDIUM)
                        .bg(if action_name == "close" {
                            vorkspace_ui::theme::danger().opacity(0.2)
                        } else if matches!(action_name.as_str(), "merge" | "squash" | "rebase") {
                            vorkspace_ui::theme::success().opacity(0.2)
                        } else {
                            content(1.)
                        })
                        .text_color(if action_name == "close" {
                            vorkspace_ui::theme::danger()
                        } else if matches!(action_name.as_str(), "merge" | "squash" | "rebase") {
                            rgb(0x6ee7b7).into()
                        } else {
                            shell_fill()
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.run_action(cx))),
                    ),
            )
            .into_any_element()
    }
}
