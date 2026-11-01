//! The work item detail page: actions, comments and activity.
use super::*;

impl Inbox {
    pub(super) fn detail_view(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.item() else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .child(
                    provider_mark(&self.state.source)
                        .size(px(24.))
                        .text_color(content(0.3)),
                )
                .child(
                    message(format!(
                        "Select a {} item",
                        model::provider_label(&self.state.source)
                    ))
                    .text_size(px(13.)),
                )
                .into_any_element();
        };
        let is_pr = item.kind == "pr";
        let details = self.details.clone().unwrap_or_default();
        let assignees = item
            .assignees
            .iter()
            .filter(|person| {
                !person
                    .login
                    .trim()
                    .eq_ignore_ascii_case(details.author.trim())
            })
            .collect::<Vec<_>>();
        let base_ref = if details.base_ref_name.is_empty() {
            self.thread
                .as_ref()
                .map(|t| t.base_ref_name.as_str())
                .unwrap_or("")
        } else {
            &details.base_ref_name
        };
        let head_ref = if details.head_ref_name.is_empty() {
            self.thread
                .as_ref()
                .map(|t| t.head_ref_name.as_str())
                .unwrap_or("")
        } else {
            &details.head_ref_name
        };
        let title = div()
            .w_full()
            .flex_shrink_0()
            .border_b_1()
            .border_color(content(0.07))
            .child(
                div()
                    .w_full()
                    .max_w(px(1024.))
                    .px(px(32.))
                    .pt(px(20.))
                    .pb(px(if is_pr { 0. } else { 20. }))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child(provider_mark(&item.provider))
                            .child(if is_pr {
                                if item.provider == "gitlab" {
                                    "Merge request"
                                } else {
                                    "Pull request"
                                }
                            } else {
                                "Issue"
                            })
                            .child(item.reference())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .text_color(status_color(&item))
                                    .child(
                                        icon(status_icon(&item), 14.)
                                            .text_color(status_color(&item)),
                                    )
                                    .child(item.status()),
                            )
                            .when(!item.attention_reason.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_color(accent_color())
                                        .child(attention_label(&item.attention_reason)),
                                )
                            })
                            .child(div().min_w_0().truncate().child(
                                if item.team_name.is_empty() {
                                    item.repo.clone()
                                } else {
                                    item.team_name.clone()
                                },
                            )),
                    )
                    .child(
                        div()
                            .line_clamp(2)
                            .text_size(px(20.))
                            .line_height(px(25.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(content(1.))
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .overflow_hidden()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .when(!details.author.is_empty(), |d| {
                                d.child(avatar::person(
                                    &item.provider,
                                    &details.author,
                                    &details.author_avatar_url,
                                    16.,
                                    window,
                                    cx,
                                ))
                            })
                            .when(!assignees.is_empty(), |d| {
                                d.child("·").children(assignees.iter().map(|person| {
                                    avatar::person(
                                        &item.provider,
                                        &person.login,
                                        &person.avatar_url,
                                        16.,
                                        window,
                                        cx,
                                    )
                                }))
                            })
                            .when(
                                item.assignees.is_empty()
                                    && matches!(item.provider.as_str(), "github" | "gitlab"),
                                |d| d.child("·").child("Unassigned"),
                            )
                            .when(!item.created_at.is_empty(), |d| {
                                d.child("·")
                                    .child(format!("Created {}", relative_time(&item.created_at)))
                            })
                            .when(!item.updated_at.is_empty(), |d| {
                                d.child("·")
                                    .child(format!("Updated {}", relative_time(&item.updated_at)))
                            })
                            .when(!base_ref.is_empty() && !head_ref.is_empty(), |d| {
                                let branch = head_ref.to_owned();
                                d.child("·").child(
                                    div()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .child(icon("git-compare", 12.).text_color(content(0.5)))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .child(format!("{base_ref} ← {head_ref}")),
                                        )
                                        .child(
                                            icon_button(
                                                "inbox-copy-branch",
                                                "Copy branch name",
                                                "response-copy",
                                            )
                                            .size(px(18.))
                                            .on_click(
                                                move |_, _, cx| {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(branch.clone()),
                                                    )
                                                },
                                            ),
                                        ),
                                )
                            })
                            .when(!details.review_decision.is_empty(), |d| {
                                d.child("·").child(review_label(&details.review_decision))
                            }),
                    )
                    .when(!self.related_sessions(&item).is_empty(), |d| {
                        d.child(
                            div()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .overflow_hidden()
                                .child(icon("message-multiple", 14.).text_color(content(0.45)))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(content(0.45))
                                        .child("Related threads"),
                                )
                                .children(self.related_sessions(&item).into_iter().map(
                                    |session| {
                                        action(
                                            format!("inbox-session-{}", session.id),
                                            session.title,
                                        )
                                        .h(px(24.))
                                        .bg(content(0.05))
                                        .max_w(px(256.))
                                        .truncate()
                                        .on_click(
                                            cx.listener(move |_, _, _, cx| {
                                                cx.emit(Event::OpenSession(session.id))
                                            }),
                                        )
                                    },
                                )),
                        )
                    })
                    .child(
                        div()
                            .pt(px(2.))
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .when(!is_pr || item.provider == "gitlab", |d| {
                                d.child(
                                    action("inbox-send-agent", "Send to chat")
                                        .px(px(12.))
                                        .bg(content(1.))
                                        .text_color(shell_fill())
                                        .font_weight(FontWeight::MEDIUM)
                                        .when(self.detail_loading || self.details.is_none(), |d| {
                                            d.opacity(0.4)
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if !this.detail_loading && this.details.is_some() {
                                                this.start(cx);
                                            }
                                        })),
                                )
                            })
                            .when(
                                (!is_pr || item.provider == "gitlab")
                                    && (item.project_path.is_empty()
                                        || matches!(item.provider.as_str(), "gitlab" | "jira")),
                                |d| {
                                    d.child(
                                        icon_action(
                                            "inbox-choose-project",
                                            &self
                                                .start_bot
                                                .as_ref()
                                                .and_then(|id| {
                                                    self.bots
                                                        .iter()
                                                        .find(|(key, _)| key == id)
                                                        .map(|(_, name)| name.clone())
                                                })
                                                .unwrap_or_else(|| {
                                                    project_name(&self.start_project)
                                                }),
                                            if self.start_bot.is_some() {
                                                "bot"
                                            } else {
                                                "folder"
                                            },
                                        )
                                        .relative()
                                        .border_1()
                                        .border_color(content(0.1))
                                        .child(icon("chevron-down", 12.))
                                        .child(self.project_anchor.probe())
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.project_picker = !this.project_picker;
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                },
                            )
                            .when(is_pr && item.provider == "github", |d| {
                                d.child(self.pr_actions(&item, cx))
                            })
                            .child(
                                icon_action(
                                    "inbox-open-external",
                                    &format!(
                                        "{} {}",
                                        if is_pr { "Review on" } else { "Open in" },
                                        model::provider_label(&item.provider)
                                    ),
                                    "external-link",
                                )
                                .when(item.url.is_empty(), |d| d.opacity(0.4))
                                .on_click({
                                    let url = item.url.clone();
                                    move |_, _, cx| {
                                        if !url.is_empty() {
                                            cx.open_url(&url);
                                        }
                                    }
                                }),
                            ),
                    )
                    .when(is_pr, |d| {
                        d.child(
                            div()
                                .id("inbox-pr-tabs")
                                .role(Role::TabList)
                                .aria_label("Pull request details")
                                .h(px(36.))
                                .flex()
                                .items_stretch()
                                .gap(px(16.))
                                .children(
                                    [
                                        (Tab::Summary, "Summary"),
                                        (Tab::Code, "Code"),
                                        (Tab::Checks, "Checks"),
                                    ]
                                    .into_iter()
                                    .filter(|(tab, _)| {
                                        *tab != Tab::Checks || item.provider == "github"
                                    })
                                    .map(|(tab, label)| {
                                        action(format!("inbox-tab-{label}"), label)
                                            .role(Role::Tab)
                                            .h_full()
                                            .px_0()
                                            .rounded_none()
                                            .when(self.tab == tab, |d| {
                                                d.border_b_2()
                                                    .border_color(content(1.))
                                                    .text_color(content(1.))
                                            })
                                            .when(tab == Tab::Checks, |d| {
                                                let failed = self
                                                    .checks
                                                    .as_ref()
                                                    .map(|checks| {
                                                        checks
                                                            .checks
                                                            .iter()
                                                            .filter(|check| {
                                                                failed_check(&check.state)
                                                            })
                                                            .count()
                                                    })
                                                    .unwrap_or(0);
                                                d.when(failed > 0, |d| {
                                                    d.child(
                                                        icon("circle-x", 13.)
                                                            .text_color(rgb(0xf87171)),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_color(rgb(0xf87171))
                                                            .child(failed.to_string()),
                                                    )
                                                })
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.set_tab(tab, cx)
                                            }))
                                    }),
                                ),
                        )
                    }),
            );
        let body = match self.tab {
            Tab::Summary => self.summary_view(&item, window, cx),
            Tab::Code => self.diff_view(cx),
            Tab::Checks => self.checks_view(cx),
        };
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(title)
            .when_some(self.detail_error.clone(), |d, text| d.child(error(text)))
            .child(body)
            .into_any_element()
    }
    pub(super) fn pr_actions(&self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let open = item.state.eq_ignore_ascii_case("open");
        let selected = self.merge_action.clone();
        let merge_label = match selected.as_str() {
            "squash" => "Squash and merge",
            "rebase" => "Rebase and merge",
            _ => "Merge pull request",
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .when(open && !item.draft, |d| {
                d.child(
                    div()
                        .relative()
                        .h(px(28.))
                        .flex()
                        .items_stretch()
                        .rounded(px(6.))
                        .bg(content(1.))
                        .text_color(shell_fill())
                        .child(self.action_anchor.probe())
                        .child(
                            icon_action_ink(
                                "inbox-pr-merge",
                                merge_label,
                                "git-merge",
                                shell_fill(),
                            )
                            .px(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(shell_fill())
                            .when(self.acting, |d| d.opacity(0.4))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !this.acting {
                                        this.confirm_action = Some(selected.clone());
                                        this.action_error = None;
                                        this.action_notice = None;
                                        cx.notify();
                                    }
                                },
                            )),
                        )
                        .child(
                            div()
                                .id("inbox-merge-options")
                                .role(Role::Button)
                                .aria_label("Merge options")
                                .h_full()
                                .w(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .border_l_1()
                                .border_color(shell_fill().opacity(0.2))
                                .cursor_pointer()
                                .child(icon("chevron-down", 12.).text_color(shell_fill()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if !this.acting {
                                        this.action_menu = !this.action_menu;
                                        cx.notify();
                                    }
                                })),
                        ),
                )
            })
            .children(
                [
                    (open && item.draft, "ready", "Ready for review"),
                    (open && !item.draft, "draft", "Convert to draft"),
                    (open, "close", "Close pull request"),
                    (
                        item.state.eq_ignore_ascii_case("closed"),
                        "reopen",
                        "Reopen pull request",
                    ),
                ]
                .into_iter()
                .filter(|(show, _, _)| *show)
                .map(|(_, action_name, label)| {
                    let anchor_ix = match action_name {
                        "ready" => 0,
                        "draft" => 1,
                        "close" => 2,
                        _ => 3,
                    };
                    icon_action(
                        format!("inbox-pr-{action_name}"),
                        label,
                        match action_name {
                            "draft" => "git-pull-request-draft",
                            "close" => "git-pull-request-closed",
                            _ => "git-pull-request",
                        },
                    )
                    .relative()
                    .child(self.pr_action_anchors[anchor_ix].probe())
                    .border_1()
                    .border_color(content(0.15))
                    .px(px(12.))
                    .when(self.acting, |d| d.opacity(0.4))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.acting {
                            this.confirm_action = Some(action_name.into());
                            this.action_error = None;
                            this.action_notice = None;
                            cx.notify();
                        }
                    }))
                }),
            )
            .when_some(self.action_notice.clone(), |d, notice| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(content(0.55))
                        .child(notice),
                )
            })
            .into_any_element()
    }

    pub(super) fn comment_card(
        &self,
        item: &Item,
        comment: &model::Comment,
        style: CommentStyle,
        width: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let nested = matches!(style, CommentStyle::Reply);
        let timeline = matches!(style, CommentStyle::Timeline);
        let has_body = !comment.body.trim().is_empty();
        let has_replies = !nested && !comment.replies.is_empty();
        let author = if comment.author.is_empty() {
            "ghost"
        } else {
            &comment.author
        };
        let reply_id = if item.provider == "github" {
            comment.thread_id.clone()
        } else {
            String::new()
        };
        let state = comment.state.trim().to_uppercase();
        let review_color = if state == "APPROVED" {
            rgb(0x4ade80).into()
        } else if state == "CHANGES_REQUESTED" {
            rgb(0xf87171).into()
        } else {
            content(0.5)
        };
        let document = self
            .comments
            .iter()
            .find(|(cached, _)| cached.id == comment.id)
            .map(|(_, document)| document.clone());
        let clamped = timeline
            && !self.expanded_comments.contains(&comment.id)
            && (comment.body.len() > 1200 || comment.body.lines().count() > 12);
        let expandable =
            timeline && (comment.body.len() > 1200 || comment.body.lines().count() > 12);
        let header = div()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.))
            .text_color(content(0.5))
            .when(!nested, |d| {
                d.px(px(12.))
                    .py(px(if timeline { 6. } else { 8. }))
                    .min_h(px(if timeline { 36. } else { 0. }))
            })
            .when(!nested && (has_body || has_replies), |d| {
                d.border_b_1().border_color(content(0.07))
            })
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(1.))
                    .child(if timeline {
                        div().child(author.to_string()).into_any_element()
                    } else {
                        avatar::person(
                            &item.provider,
                            author,
                            &comment.author_avatar_url,
                            20.,
                            window,
                            cx,
                        )
                    }),
            )
            .when(!state.is_empty(), |d| {
                d.child("·")
                    .child(div().text_color(review_color).child(review_label(&state)))
            })
            .when(!comment.path.is_empty(), |d| {
                d.child("·").child(div().min_w_0().truncate().child(format!(
                        "{}{}",
                        comment.path,
                        comment
                            .line
                            .map(|line| format!(":{line}"))
                            .unwrap_or_default()
                    )))
            })
            .when(comment.resolved, |d| {
                d.child("·")
                    .child(div().text_color(rgb(0x4ade80)).child("Resolved"))
            })
            .when(!comment.created_at.is_empty(), |d| {
                d.child("·").child(
                    action(
                        format!("inbox-comment-time-{}", comment.id),
                        relative_time(&comment.created_at),
                    )
                    .h_auto()
                    .px_0()
                    .text_color(content(0.5))
                    .on_click({
                        let url = comment.url.clone();
                        move |_, _, cx| {
                            if !url.is_empty() {
                                cx.open_url(&url);
                            }
                        }
                    }),
                )
            })
            .when(!reply_id.is_empty(), |d| {
                let name = author.to_string();
                d.child("·").child(
                    action(format!("inbox-reply-{}", comment.id), "Reply")
                        .h_auto()
                        .px_0()
                        .text_color(content(0.5))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reply = Some((reply_id.clone(), name.clone()));
                            this.summary_scroll.scroll_to_end();
                            window.focus(&this.comment.focus_handle(cx), cx);
                            cx.notify();
                        })),
                )
            });
        div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .when(!nested, |d| {
                d.rounded(px(6.))
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.05))
                    .overflow_hidden()
            })
            .child(header)
            .when(has_body, |d| {
                d.child(
                    div()
                        .when(nested, |d| d.mt(px(8.)))
                        .when(!nested, |d| d.px(px(12.)).py(px(10.)))
                        .child(
                            div()
                                .id(format!("inbox-comment-body-{}", comment.id))
                                .w_full()
                                .when(clamped, |d| d.max_h(px(180.)).overflow_hidden())
                                .when_some(document, |d, document| {
                                    d.child(document::DocumentElement::new(
                                        document,
                                        if nested {
                                            width
                                        } else {
                                            (width - px(26.)).max(px(1.))
                                        },
                                        clamped.then_some(px(180.)),
                                    ))
                                }),
                        )
                        .when(expandable, |d| {
                            let id = comment.id.clone();
                            d.child(
                                action(
                                    format!("inbox-expand-comment-{}", comment.id),
                                    if clamped { "Show more" } else { "Show less" },
                                )
                                .mt(px(6.))
                                .h_auto()
                                .px_0()
                                .text_color(content(0.5))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if !this.expanded_comments.remove(&id) {
                                            this.expanded_comments.insert(id.clone());
                                        }
                                        this.summary_scroll.remeasure();
                                        cx.notify();
                                    },
                                )),
                            )
                        }),
                )
            })
            .when(has_replies, |d| {
                d.child(
                    div()
                        .border_t_1()
                        .border_color(content(0.07))
                        .px(px(12.))
                        .children(comment.replies.iter().enumerate().map(|(ix, reply)| {
                            div()
                                .py(px(10.))
                                .when(ix > 0, |d| d.border_t_1().border_color(content(0.07)))
                                .child(self.comment_card(
                                    item,
                                    reply,
                                    CommentStyle::Reply,
                                    (width - px(26.)).max(px(1.)),
                                    window,
                                    cx,
                                ))
                        })),
                )
            })
            .into_any_element()
    }

    pub(super) fn timeline_rail(
        &self,
        node: AnyElement,
        first: bool,
        last: bool,
        card: bool,
    ) -> Div {
        div()
            .w(px(20.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w(px(1.))
                    .h(px(if card { 8. } else { 0. }))
                    .flex_shrink_0()
                    .when(!first, |d| d.bg(content(0.1))),
            )
            .child(
                div()
                    .size(px(20.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(node),
            )
            .when(!last, |d| {
                d.child(div().w(px(1.)).flex_1().bg(content(0.1)))
            })
    }

    pub(crate) fn rebuild_activity(&mut self) {
        self.activity.clear();
        self.activity_revision += 1;
        let Some(thread) = &self.thread else {
            return;
        };
        let timeline = self
            .item()
            .is_some_and(|item| item.kind == "pr" && item.provider == "github");
        if timeline {
            let mut entries = thread
                .comments
                .iter()
                .map(|comment| {
                    (
                        model::timestamp_ms(&comment.created_at),
                        Activity::Comment(std::sync::Arc::new(comment.clone())),
                    )
                })
                .chain(thread.commits.iter().map(|commit| {
                    (
                        model::timestamp_ms(&commit.committed_date),
                        Activity::Commits(vec![commit.clone()]),
                    )
                }))
                .collect::<Vec<_>>();
            entries.sort_by_key(|(at, _)| *at);
            let mut groups: Vec<Activity> = Vec::new();
            for (_, entry) in entries {
                match entry {
                    Activity::Commits(mut commits) => {
                        if let Some(Activity::Commits(previous)) = groups.last_mut()
                            && previous
                                .last()
                                .is_some_and(|c| c.author == commits[0].author)
                        {
                            previous.append(&mut commits);
                            continue;
                        }
                        groups.push(Activity::Commits(commits));
                    }
                    entry => groups.push(entry),
                }
            }
            self.activity = groups;
        } else {
            self.activity = thread
                .comments
                .iter()
                .cloned()
                .map(std::sync::Arc::new)
                .map(Activity::Comment)
                .collect();
        }
        self.summary_scroll.remeasure();
    }

    pub(super) fn discussion_row(
        &self,
        item: &Item,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entry = self.activity[ix].clone();
        let timeline = item.kind == "pr" && item.provider == "github";
        let width = self.summary_body_width();
        if !timeline {
            let Activity::Comment(comment) = entry else {
                unreachable!()
            };
            return div()
                .pb(px(if ix + 1 == self.activity.len() {
                    0.
                } else {
                    8.
                }))
                .child(self.comment_card(item, &comment, CommentStyle::Plain, width, window, cx))
                .into_any_element();
        }
        let first = ix == 0;
        let last = ix + 1 == self.activity.len();
        let mut activity = Vec::new();
        match entry {
            Activity::Comment(comment) => {
                let event = review_event(&comment);
                let state = comment.state.to_uppercase();
                let color = if state == "APPROVED" {
                    rgb(0x4ade80).into()
                } else if state == "CHANGES_REQUESTED" {
                    rgb(0xf87171).into()
                } else {
                    content(0.45)
                };
                let node = if event {
                    icon(
                        if state == "APPROVED" {
                            "check-circle"
                        } else if state == "CHANGES_REQUESTED" {
                            "circle-x"
                        } else {
                            "message-multiple"
                        },
                        16.,
                    )
                    .text_color(color)
                    .into_any_element()
                } else {
                    avatar::image(
                        &item.provider,
                        &comment.author,
                        &comment.author_avatar_url,
                        20.,
                        window,
                        cx,
                    )
                };
                let body = if event {
                    let verb = match state.as_str() {
                        "APPROVED" => "approved",
                        "CHANGES_REQUESTED" => "requested changes",
                        "DISMISSED" => "had a review dismissed",
                        _ => "reviewed",
                    };
                    timeline_event(&comment.author, verb, &comment.created_at).into_any_element()
                } else {
                    self.comment_card(
                        item,
                        &comment,
                        CommentStyle::Timeline,
                        (width - px(32.)).max(px(1.)),
                        window,
                        cx,
                    )
                };
                activity.push(
                    div()
                        .min_w_0()
                        .flex()
                        .gap(px(12.))
                        .child(self.timeline_rail(node, first, last, !event))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .when(!last, |d| d.pb(px(12.)))
                                .child(body),
                        )
                        .into_any_element(),
                );
            }
            Activity::Commits(commits) => {
                let author = commits[0].author.clone();
                let time = commits
                    .last()
                    .map(|commit| commit.committed_date.clone())
                    .unwrap_or_default();
                let node = avatar::image(&item.provider, &author, "", 20., window, cx);
                activity.push(
                    div()
                        .min_w_0()
                        .flex()
                        .gap(px(12.))
                        .child(self.timeline_rail(node, first, false, false))
                        .child(div().min_w_0().flex_1().pb(px(12.)).child(timeline_event(
                            &author,
                            &format!(
                                "added {}",
                                if commits.len() == 1 {
                                    "a commit".into()
                                } else {
                                    format!("{} commits", commits.len())
                                }
                            ),
                            &time,
                        )))
                        .into_any_element(),
                );
                let commit_count = commits.len();
                for (commit_ix, commit) in commits.into_iter().enumerate() {
                    let final_commit = last && commit_ix + 1 == commit_count;
                    let url = commit.url.clone();
                    activity.push(
                        div()
                            .min_w_0()
                            .flex()
                            .gap(px(12.))
                            .child(
                                div()
                                    .w(px(20.))
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .child(div().h(px(6.)).w(px(1.)).bg(content(0.1)))
                                    .child(
                                        div()
                                            .size(px(8.))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(content(0.35)),
                                    )
                                    .when(!final_commit, |d| {
                                        d.child(div().w(px(1.)).flex_1().bg(content(0.1)))
                                    }),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .when(!final_commit, |d| d.pb(px(12.)))
                                    .child(
                                        div()
                                            .id(format!("inbox-commit-{}", commit.oid))
                                            .role(Role::Button)
                                            .aria_label("Open commit")
                                            .h(px(20.))
                                            .flex()
                                            .items_center()
                                            .gap(px(12.))
                                            .text_size(px(12.))
                                            .cursor_pointer()
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .flex_1()
                                                    .truncate()
                                                    .text_color(content(0.7))
                                                    .child(commit.message_headline),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .font_family("monospace")
                                                    .text_color(content(0.35))
                                                    .child(
                                                        commit
                                                            .oid
                                                            .chars()
                                                            .take(7)
                                                            .collect::<String>(),
                                                    ),
                                            )
                                            .on_click(move |_, _, cx| {
                                                if !url.is_empty() {
                                                    cx.open_url(&url);
                                                }
                                            }),
                                    ),
                            )
                            .into_any_element(),
                    );
                }
            }
        }
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .children(activity)
            .into_any_element()
    }

    pub(super) fn discussion_heading(
        &self,
        item: &Item,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let thread = self.thread.as_ref()?;
        if thread.comments.is_empty() && thread.commits.is_empty() && !thread.truncated {
            return None;
        }
        let timeline = item.kind == "pr" && item.provider == "github";
        let count = thread
            .comments
            .iter()
            .filter(|comment| !(timeline && review_event(comment)))
            .map(|comment| 1 + comment.replies.len())
            .sum::<usize>();
        Some(
            div()
                .w_full()
                .flex_shrink_0()
                .border_t_1()
                .border_color(content(0.07))
                .pt(px(20.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .child(div().text_color(content(0.7)).child(if timeline {
                            "Activity".into()
                        } else {
                            format!(
                                "{count} {}",
                                if count == 1 { "comment" } else { "comments" }
                            )
                        }))
                        .when(timeline, |d| {
                            d.child(format!(
                                "{count} {}{}",
                                if count == 1 { "comment" } else { "comments" },
                                if thread.commits.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        " · {} {}",
                                        thread.commits.len(),
                                        if thread.commits.len() == 1 {
                                            "commit"
                                        } else {
                                            "commits"
                                        }
                                    )
                                }
                            ))
                        })
                        .when(thread.truncated, |d| {
                            d.child(format!(
                                "Latest comments · more on {}",
                                model::provider_label(&item.provider)
                            ))
                        }),
                )
                .when_some(self.thread_error.clone(), |d, text| d.child(error(text)))
                .into_any_element(),
        )
    }
}
