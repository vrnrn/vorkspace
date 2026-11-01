//! Rendering each large row kind. `row_body` chooses among these and adds the
//! layout every row shares.
use super::*;
use gpui::AnyElement;

impl TranscriptView {
    pub(super) fn interjection_row(
        &mut self,
        note: &vorkspace_core::transcript::Interjection,
        message_start: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.expanded.contains(&message_start);
        let long = note.text.lines().count() > 4 || note.text.len() > 300;
        let severity = note.severity.as_deref();
        let color = match severity {
            Some("blocker") => rgb(0xf87171).into(),
            Some("concern") => rgb(0xfbbf24).into(),
            _ => content(0.55),
        };
        div()
            .w_full()
            .min_w_0()
            .py_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().h(px(1.)).flex_1().bg(content(0.12)))
                    .child(
                        div()
                            .id(("interjection-label", message_start))
                            .role(Role::Group)
                            .aria_label(format!("Interjection: {}", note.label()))
                            .text_size(px(12.))
                            .text_color(content(0.55))
                            .child(note.label().to_owned()),
                    )
                    .when_some(severity, |d, severity| {
                        let label = match severity {
                            "blocker" => "Blocker",
                            "concern" => "Concern",
                            "nit" => "Nit",
                            value => value,
                        };
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(color)
                                .child(label.to_owned()),
                        )
                    })
                    .child(div().h(px(1.)).flex_1().bg(content(0.12))),
            )
            .when(!note.text.is_empty(), |d| {
                d.child(
                    div()
                        .w_full()
                        .text_size(px(12.5))
                        .line_height(px(20.))
                        .text_color(content(0.7))
                        .when(!open, |d| d.max_h(px(80.)).overflow_hidden())
                        .child(note.text.clone()),
                )
            })
            .when(long, |d| {
                d.child(
                    div()
                        .id(("interjection-expand", message_start))
                        .role(Role::Button)
                        .aria_expanded(open)
                        .text_size(px(11.))
                        .text_color(content(0.5))
                        .child(if open { "Show less" } else { "Show more" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle(message_start, cx);
                        })),
                )
            })
            .into_any_element()
    }
    pub(super) fn work_line_row(
        &mut self,
        turn: usize,
        expandable: bool,
        message_start: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let live = self.streaming
            && message_start >= self.live_start
            && self.source[message_start..].iter().all(|r| !r.is_prompt());
        let elapsed = if live {
            self.started.map(|at| at.elapsed())
        } else {
            self.elapsed.get(&turn).copied().or_else(|| {
                self.turns
                    .get(&turn)
                    .filter(|info| info.has_duration())
                    .map(|info| std::time::Duration::from_millis(info.duration_ms))
            })
        };
        let model = self
            .turns
            .get(&turn)
            .map(|info| info.model.trim())
            .filter(|model| !model.is_empty())
            .or_else(|| live.then_some(self.model.as_str()));
        let title = work_title(
            model,
            elapsed.map(|duration| duration.as_millis() as u64),
            live,
        );
        let open = self.work_folds.get(&turn).copied().unwrap_or(false);
        let label = if expandable {
            if open {
                "Hide the work".into()
            } else {
                "Show the work".into()
            }
        } else {
            title.clone()
        };
        div()
            .id(("work-fold", turn))
            .debug_selector(move || format!("work-fold-{turn}"))
            .role(if expandable {
                Role::Button
            } else {
                Role::Label
            })
            .aria_label(label)
            .aria_expanded(open)
            .group("work-fold")
            .flex()
            .items_center()
            .gap_1p5()
            .py_1()
            .text_size(px(14.))
            .line_height(px(20.))
            .text_color(content(0.5))
            .child(
                div()
                    .relative()
                    .size(px(14.))
                    .flex_shrink_0()
                    .when(!open, |d| {
                        d.child(
                            provider_icon(
                                self.turns
                                    .get(&turn)
                                    .map(|info| info.provider.as_str())
                                    .filter(|provider| !provider.is_empty())
                                    .unwrap_or(self.provider.as_str()),
                                14.,
                            )
                            .when(expandable, |i| {
                                i.group_hover("work-fold", |s| s.opacity(0.))
                            }),
                        )
                    })
                    .when(expandable, |d| {
                        d.child(
                            icon(
                                if open {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                },
                                14.,
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .when(!open, |i| {
                                i.opacity(0.).group_hover("work-fold", |s| s.opacity(1.))
                            }),
                        )
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .when(live, |d| d.debug_selector(|| "live-work-receipt".into()))
                    .child(if live {
                        crate::activity::shimmer(
                            title,
                            self.started.map_or(0., |s| s.elapsed().as_secs_f32()),
                        )
                    } else {
                        div().child(title).into_any_element()
                    }),
            )
            .when(expandable, |d| d.hover(|s| s.text_color(content(0.8))))
            .on_click(cx.listener(move |this, _, _, cx| {
                if expandable {
                    this.work_folds.insert(turn, !open);
                    this.list.pause_following_tail();
                    this.rebuild(turn, cx);
                }
            }))
            .into_any_element()
    }
    pub(super) fn phase_row(
        &mut self,
        title: String,
        group: usize,
        glyph: &'static str,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.phases.get(&group).copied().unwrap_or(active);
        let rotation = self
            .motions
            .values()
            .find(|m| m.group == Some(group))
            .map_or(if open { 1. } else { 0. }, |m| m.rotation(self.frame_now));
        div()
            .id(("phase", group))
            .role(Role::Button)
            .aria_label(format!(
                "{} the steps for {title}",
                if open { "Hide" } else { "Show" }
            ))
            .aria_expanded(open)
            .group("activity-phase")
            .flex()
            .items_center()
            .gap_1p5()
            .py_1()
            .text_size(px(14.))
            .line_height(px(20.))
            .text_color(content(0.5))
            .child(
                div()
                    .relative()
                    .size(px(14.))
                    .flex_shrink_0()
                    .child(if active {
                        // The compaction step's dots stand in for the
                        // group's icon until its work settles.
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                vorkspace_ui::session_mark::spinner_dots(0).with_animation(
                                    ("phase-loader", group),
                                    Animation::new(std::time::Duration::from_millis(1_200))
                                        .repeat()
                                        .with_max_fps(30.),
                                    |_, phase| {
                                        vorkspace_ui::session_mark::spinner_dots(
                                            (phase * 1_200.) as u64,
                                        )
                                    },
                                ),
                            )
                            .group_hover("activity-phase", |s| s.opacity(0.))
                            .into_any_element()
                    } else {
                        icon(glyph, 14.)
                            .group_hover("activity-phase", |s| s.opacity(0.))
                            .into_any_element()
                    })
                    .child(
                        icon("chevron-right", 14.)
                            .with_transformation(Transformation::rotate(percentage(
                                rotation * 0.25,
                            )))
                            .absolute()
                            .top_0()
                            .left_0()
                            .opacity(0.)
                            .group_hover("activity-phase", |s| s.opacity(1.)),
                    ),
            )
            .child(div().min_w_0().flex_1().truncate().child(if active {
                crate::activity::shimmer(
                    title,
                    self.started.map_or(0., |s| s.elapsed().as_secs_f32()),
                )
            } else {
                div().child(title).into_any_element()
            }))
            .hover(|s| s.text_color(content(0.8)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_phase(group, !open, cx);
            }))
            .into_any_element()
    }
    pub(super) fn reasoning_row(
        &mut self,
        row: &UiRow,
        message_start: usize,
        nested_phase: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let summary = self.messages[row.message]
            .parser
            .tree()
            .blocks
            .iter()
            .flat_map(|block| markdown::rows(&block.block))
            .map(|row| markdown::row_text(&row))
            .find(|text| !text.trim().is_empty())
            .unwrap_or_else(|| "Thinking…".into())
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let open = self.expanded.contains(&message_start);
        div()
            .id(("thinking", message_start))
            .debug_selector(move || format!("thinking-{message_start}"))
            .role(Role::Button)
            .aria_label(format!(
                "{} thinking: {summary}",
                if open { "Hide" } else { "Show" }
            ))
            .aria_expanded(open)
            .flex()
            .items_center()
            .gap_1p5()
            .py_1()
            .line_height(px(20.))
            .text_size(px(14.))
            .text_color(content(0.5))
            .hover(|s| s.text_color(content(0.8)))
            .when(!nested_phase, |d| d.child(icon("minus", 14.)))
            .child(div().min_w_0().flex_1().truncate().child(if open {
                "Thinking".into()
            } else {
                summary
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(message_start, cx)))
            .into_any_element()
    }
    pub(super) fn markdown_row(
        &mut self,
        md: &MarkdownRow,
        row: &UiRow,
        ix: usize,
        look: MessageLook,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let MessageLook {
            user,
            error,
            reasoning,
            message_start,
            after_receipt,
            first_prose,
            ..
        } = look;
        let resolved = (self.file_link_base.is_some() || self.search_match.is_some()).then(|| {
            let mut md = md.clone();
            if let Some(base) = &self.file_link_base {
                markdown::resolve_file_links(&mut md, base);
            }
            if let Some((current, query)) = &self.search_match {
                markdown::mark_row(&mut md, query, *current == message_start);
            }
            md
        });
        let md = resolved.as_ref().unwrap_or(md);
        let body = if let Part::Code { fence, lines } = &md.part {
            self.code(fence.clone(), lines.clone(), ix, cx)
        } else {
            markdown::body_selected(
                md,
                format!("md-{ix}").into(),
                if reasoning {
                    markdown::Tone::Reasoning
                } else if row.work {
                    markdown::Tone::Work
                } else {
                    markdown::Tone::Normal
                },
                self.reveals
                    .get(&message_start)
                    .map(|r| r.births.as_slice()),
                self.frame_now,
                if reasoning {
                    None
                } else {
                    self.markdown_selection(row.key, message_start, !user && !error, cx)
                },
            )
            .into_any_element()
        };
        div()
            .w_full()
            .flex()
            .pt(px(if first_prose {
                if after_receipt || row.work { 4. } else { 12. }
            } else if row.work {
                md.gap.min(12.)
            } else {
                md.gap
            }))
            .pl(px(md.indent.saturating_sub(1) as f32 * 24.))
            .when(md.quote > 0, |d| {
                d.ml(px(4.)).pl_4().border_l_4().border_color(content(0.2))
            })
            .when(md.indent > 0, |d| {
                d.child(
                    div()
                        .w(px(24.))
                        .flex_shrink_0()
                        .text_color(content(0.78))
                        .child(md.marker.clone().unwrap_or_default()),
                )
            })
            .child(body)
            .into_any_element()
    }
    pub(super) fn plain_row(
        &mut self,
        text: SharedString,
        row: &UiRow,
        attachment_body: Option<AnyElement>,
        look: MessageLook,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let MessageLook {
            user,
            stacked,
            error,
            message_start,
            first,
            last,
            ..
        } = look;
        let collapsed = !self.expanded_prompts.contains(&message_start);
        let clamp = user && first && collapsed;
        // `overflows`: hidden chunks, or more than four wrapped lines.
        let overflows = user
            && last
            && (!first
                || self.clamped_prompts.contains(&message_start)
                || self.prompt_lines(&text, window) > 4);
        div()
            .w_full()
            .relative()
            .flex()
            .flex_col()
            .text_color(if error {
                rgb(0xf87171).into()
            } else {
                content(1.)
            })
            .when(user, |d| {
                // AgentTranscript.tsx: text-sm (14px / 20px), px-3 py-2.
                d.line_height(px(20.))
                    .px_3()
                    .pt(px(if first { 8. } else { 0. }))
                    .pb(px(if last { 8. } else { 0. }))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                // Match React's measured single-line bubble, including
                                // when a window resize makes the same text wrap.
                                let radius = if first && last && bounds.size.height <= px(37.) {
                                    f32::from(bounds.size.height) * 0.5
                                } else {
                                    12.
                                };
                                window.paint_quad(
                                    gpui::fill(bounds, user_bubble_fill()).corner_radii(
                                        gpui::Corners {
                                            top_left: px(if first { radius } else { 0. }),
                                            top_right: px(if first { radius } else { 0. }),
                                            bottom_left: px(if last { radius } else { 0. }),
                                            bottom_right: px(if last { radius } else { 0. }),
                                        },
                                    ),
                                );
                            },
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
            })
            .children(
                attachment_body.map(|body| div().when(!text.is_empty(), |d| d.mb_2()).child(body)),
            )
            .when(!text.is_empty(), |d| {
                d.child(
                    div()
                        .when(clamp, |d| d.line_clamp(4).text_ellipsis())
                        .child({
                            let styled = gpui::StyledText::new(text.clone()).with_highlights(
                                markdown::mark_styles(
                                    &text,
                                    vec![],
                                    self.search_match.as_ref().map_or("", |(_, q)| q.as_str()),
                                    self.search_match
                                        .as_ref()
                                        .is_some_and(|(start, _)| *start == message_start),
                                ),
                            );
                            let layout = styled.layout().clone();
                            let child = styled.into_any_element();
                            if let Some(selection) =
                                self.text_selection(row.key, message_start, false, cx)
                            {
                                selection.wrap(text.clone(), layout, child)
                            } else {
                                child
                            }
                        }),
                )
            })
            .when(overflows, |d| {
                d.child(
                    div().flex().child(
                        div()
                            .id(("prompt-more", message_start))
                            .role(Role::Button)
                            .aria_expanded(!collapsed)
                            .mt_1()
                            .px_1()
                            .py(px(2.))
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(content(0.6))
                            .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                            .child(if collapsed { "Show more" } else { "Show less" })
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation()
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_prompt(message_start, cx)
                            })),
                    ),
                )
            })
            .when(user && self.bot && first, |d| {
                d.children(self.reaction_badge(message_start))
            })
            .when(
                user && last
                    && !stacked
                    && (self.hovered_prompt == Some(message_start)
                        || self.hovered_prompt_actions == Some(message_start)),
                |d| d.child(self.prompt_actions(message_start, cx)),
            )
            .into_any_element()
    }
}

/// The facts about a row's message that its renderers share.
#[derive(Clone, Copy)]
pub(super) struct MessageLook {
    pub user: bool,
    pub stacked: bool,
    pub error: bool,
    pub reasoning: bool,
    pub message_start: usize,
    pub first: bool,
    pub last: bool,
    pub after_receipt: bool,
    pub first_prose: bool,
}
