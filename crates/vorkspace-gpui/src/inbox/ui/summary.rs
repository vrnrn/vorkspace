//! The summary, diff and checks tabs of a work item.
use super::*;
use vorkspace_ui::radius;

impl Inbox {
    pub(super) fn summary_body_width(&self, window: &Window) -> Pixels {
        let inset = self.detail_inset(window);
        (self.summary_width.get().min(px(page::DETAIL_MAX)) - inset - inset).max(px(1.))
    }

    pub(super) fn summary_view(
        &self,
        item: &Item,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let discussion = self.thread.as_ref().is_some_and(|thread| {
            !thread.comments.is_empty() || !thread.commits.is_empty() || thread.truncated
        });
        let count = 2 + if discussion {
            1 + self.activity.len()
        } else {
            0
        };
        let mut rows = self.summary_rows.borrow_mut();
        if rows.keys.is_empty()
            || rows.revision != self.activity_revision
            || rows.discussion != discussion
        {
            let mut keys: Vec<SharedString> = vec!["intro".into()];
            if discussion {
                keys.push("activity".into());
                keys.extend(self.activity.iter().map(|activity| match activity {
                    Activity::Comment(comment) => format!("comment:{}", comment.id).into(),
                    Activity::Commits(commits) => format!("push:{}", commits[0].oid).into(),
                }));
            }
            keys.push("composer".into());
            // Retain unchanged prefix/suffix rows so refreshes preserve the
            // reader's item and pixel offset, including a focused composer.
            let prefix = rows
                .keys
                .iter()
                .zip(&keys)
                .take_while(|(old, new)| old == new)
                .count();
            let suffix = rows.keys[prefix..]
                .iter()
                .rev()
                .zip(keys[prefix..].iter().rev())
                .take_while(|(old, new)| old == new)
                .count();
            self.summary_scroll.splice_focusable(
                prefix..rows.keys.len() - suffix,
                (prefix..keys.len() - suffix)
                    .map(|ix| (ix + 1 == count).then(|| self.comment.focus_handle(cx))),
            );
            *rows = SummaryRows {
                revision: self.activity_revision,
                discussion,
                keys,
            };
        }
        drop(rows);
        let width = self.summary_width.clone();
        let entity = cx.entity();
        let item = item.clone();
        div()
            .id(format!("inbox-summary-{}", item.key()))
            .relative()
            .min_h_0()
            .min_w_0()
            .flex_1()
            .child(
                canvas(
                    move |bounds, _, _| {
                        width.set(bounds.size.width);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                list(self.summary_scroll.clone(), move |ix, window, cx| {
                    entity.update(cx, |this, cx| {
                        this.summary_row(&item, ix, count, discussion, window, cx)
                    })
                })
                .size_full(),
            )
            .into_any_element()
    }

    pub(super) fn summary_row(
        &self,
        item: &Item,
        ix: usize,
        count: usize,
        discussion: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_post = !self.posting
            && self.detail_target_error.is_none()
            && !self.comment.read(cx).content.trim().is_empty();
        let body = if ix == 0 {
            div()
                .py(self.detail_inset(window))
                .flex()
                .flex_col()
                .gap(px(16.))
                .when(!item.labels.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .children(item.labels.iter().map(|label| label_badge(label, false))),
                    )
                })
                .when(self.detail_loading && self.details.is_none(), |d| {
                    d.child(message("Loading description…"))
                })
                .when(
                    self.details
                        .as_ref()
                        .is_some_and(|d| d.body.trim().is_empty()),
                    |d| d.child(page::meta().child("No description")),
                )
                .when_some(self.document.clone(), |d, document| {
                    d.child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .child(document::DocumentElement::new(
                                document,
                                self.summary_body_width(window),
                                None,
                            )),
                    )
                })
                .when(self.thread.is_none(), |d| {
                    d.when_some(self.thread_error.clone(), |d, text| d.child(error(text)))
                })
                .into_any_element()
        } else if ix + 1 == count {
            div()
                .pb(self.detail_inset(window))
                .child(
                    div()
                        .w_full()
                        .flex_shrink_0()
                        .border_t_1()
                        .border_color(page::divider())
                        .pt(px(16.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .when_some(self.reply.clone(), |d, (_, author)| {
                            d.child(
                                page::meta()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .child(format!("Replying to {author}")),
                                    )
                                    .child(
                                        icon_button("inbox-cancel-reply", "Cancel reply", "close")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.reply = None;
                                                this.post_error = None;
                                                cx.notify();
                                            })),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .rounded(px(radius::SURFACE))
                                .border_1()
                                .border_color(page::divider())
                                .bg(content(0.05))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .px(px(12.))
                                        .py(px(8.))
                                        .when(self.posting, |d| d.opacity(0.4))
                                        .child(self.comment.clone()),
                                )
                                .child({
                                    let label = if self.posting {
                                        "Posting…"
                                    } else if self.reply.is_some() {
                                        "Reply"
                                    } else {
                                        "Comment"
                                    };
                                    div().px(px(8.)).pb(px(8.)).flex().justify_end().child(
                                        page::primary("inbox-post-comment", label, !can_post)
                                            .child(label)
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.post_comment(cx)),
                                            ),
                                    )
                                }),
                        )
                        .when_some(self.post_error.clone(), |d, text| d.child(error(text))),
                )
                .into_any_element()
        } else if discussion && ix == 1 {
            div()
                .pb(px(if self.activity.is_empty() { 16. } else { 12. }))
                .children(self.discussion_heading(item, window, cx))
                .into_any_element()
        } else {
            div()
                .pb(px(if ix + 2 == count { 16. } else { 0. }))
                .child(self.discussion_row(item, ix - 2, window, cx))
                .into_any_element()
        };
        div()
            .id(("inbox-summary-row", ix))
            .w_full()
            .max_w(px(page::DETAIL_MAX))
            .mx_auto()
            .px(self.detail_inset(window))
            .child(body)
            .into_any_element()
    }

    pub(super) fn diff_view(&self, _cx: &mut Context<Self>) -> AnyElement {
        div()
            .min_h_0()
            .flex_1()
            .flex()
            .flex_col()
            .when(self.extra_loading, |d| d.child(message("Loading changes…")))
            .when_some(self.extra_error.clone(), |d, text| d.child(error(text)))
            .when(self.diff.is_some(), |d| {
                d.child(div().flex_1().min_h_0().child(self.diff_view.clone()))
            })
            .when(self.diff.as_ref().is_some_and(|diff| diff.truncated), |d| {
                d.child(message(
                    "This diff is truncated. Open the pull request to see all changes.",
                ))
            })
            .into_any_element()
    }

    pub(super) fn checks_view(&self, inset: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let body = div()
            .w_full()
            .max_w(px(page::DETAIL_MAX))
            .mx_auto()
            .p(inset)
            .when(self.extra_loading, |d| d.child(message("Loading checks…")))
            .when_some(self.extra_error.clone(), |d, text| d.child(error(text)))
            .when_some(self.checks.as_ref(), |d, checks| {
                let failed = checks
                    .checks
                    .iter()
                    .filter(|check| failed_check(&check.state))
                    .cloned()
                    .collect::<Vec<_>>();
                d.when(checks.checks.is_empty(), |d| {
                    d.child(page::empty(
                        action_icon("check-circle", page::secondary_ink()).size(px(24.)),
                        "No checks reported",
                        "",
                    ))
                })
                .when(!failed.is_empty(), |d| {
                    d.child(
                        page::meta()
                            .mb(px(12.))
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().child(format!("{} failing checks", failed.len())))
                            .child(
                                page::secondary("inbox-fix-checks", "Fix with agent", false, false)
                                    .child("Fix with agent")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(mut item) = this.item() {
                                            if item.project_path.is_empty() {
                                                item.project_path =
                                                    this.start_project.to_string_lossy().into_owned();
                                            }
                                            let evidence = failed
                                                .iter()
                                                .map(|check| format!(
                                                    "{}: {} {}",
                                                    check.name,
                                                    check.state,
                                                    check.url.as_deref().unwrap_or("")
                                                ))
                                                .collect::<Vec<_>>()
                                                .join("\n");
                                            let prompt = format!("Investigate and repair the failing CI checks for {} {}: {}\n{}\n\n{}\n\nRead the check logs, reproduce the failures, implement the fix, and run relevant checks.", item.repo, item.reference(), item.title, item.url, evidence);
                                            cx.emit(Event::StartSession {
                                                item: Box::new(item),
                                                prompt,
                                                bot: None,
                                            });
                                        }
                                    })),
                            ),
                    )
                })
                .children(checks.checks.iter().enumerate().map(|(ix, check)| {
                    let state = check.state.to_lowercase();
                    let color: Hsla = if matches!(state.as_str(), "success" | "neutral" | "skipped") {
                        status(StatusColor::Success, 1.)
                    } else if failed_check(&state) {
                        status(StatusColor::Danger, 1.)
                    } else {
                        status(StatusColor::Warning, 1.)
                    };
                    div()
                        .id(("inbox-check", ix))
                        .py(px(12.))
                        .border_b_1()
                        .border_color(page::divider())
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(action_icon(
                            if state == "success" { "check-circle" } else { "circle-dot" },
                            color,
                        ))
                        .child(
                            div().flex_1().min_w_0()
                                .child(page::label().child(check.name.clone()))
                                .child(page::meta().child(check.workflow.clone())),
                        )
                        .child(
                            page::meta()
                                .text_color(color)
                                .child(state.replace('_', " ")),
                        )
                        .when_some(check.url.clone(), |d, url| {
                            d.child(
                                action(format!("inbox-check-link-{ix}"), "Details ↗")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        })
                }))
            });
        div()
            .id("inbox-checks-scroll")
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .child(body)
            .into_any_element()
    }
}
