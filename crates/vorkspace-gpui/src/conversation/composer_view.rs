//! Rendering the composer and its tool row.
use super::*;

impl Conversation {
    /// `Session: Stop` on Escape: cancels a running turn.
    pub(crate) fn stop_if_busy(&mut self, cx: &mut Context<Self>) {
        if self.cancel_edit_last_turn(cx) {
            return;
        }
        if self.busy || self.ticket_context.pending() {
            self.stop(cx);
        }
    }
    pub(super) fn stop(&mut self, cx: &mut Context<Self>) {
        let ticket_pending = self.ticket_context.pending();
        self.ticket_context.cancel();
        if ticket_pending {
            self.status = "Ready".into();
            cx.notify();
        }
        if !self.busy || self.stopping {
            return;
        }
        self.stopping = true;
        self.busy = false;
        self.follow_ups.resuming = false;
        self.follow_ups.continuations.clear();
        self.notification_cancelled = true;
        self.cancel.store(true, Ordering::Relaxed);
        if !self.follow_ups.state.messages.is_empty() {
            self.follow_ups.state.paused = true;
            self.follow_ups.requested = None;
            self.save_follow_ups(cx);
        }
        self.follow_ups.ready = false;
        self.approvals.clear();
        self.questions.clear();
        self.controls = None;
        self.status = "Ready".into();
        self.transcript
            .update(cx, |view, cx| view.stop_streaming(cx));
        self.review
            .update(cx, |review, cx| review.set_busy(false, cx));
        cx.emit(SessionChanged);
        cx.notify();
    }
    // Composer.tsx in the inset layout: a frosted, rounded box holding the
    // field and its toolbar, with the workspace bar sitting under it.
    pub(super) fn composer(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let command = if self.follow_ups.draft_selected {
            Some(input::Command::Draft)
        } else if self.settings.orchestrator {
            Some(input::Command::Orchestrator)
        } else if self.settings.plan {
            Some(input::Command::Plan)
        } else if self.settings.operator {
            Some(input::Command::Operator)
        } else {
            None
        };
        self.input
            .update(cx, |input, cx| input.selected_command(command, cx));
        self.completion.update(cx, |completion, cx| {
            completion.set_cwd(
                self.settings
                    .worktree_cwd
                    .as_deref()
                    .unwrap_or(&self.summary.cwd),
                cx,
            )
        });
        let menu: Vec<_> = self
            .menu
            .as_ref()
            .map(|open| {
                menu_popover(&open.menu, Anchor::Slot(self.anchor(open.kind).clone()), cx)
                    .side(Side::Top)
                    .when(open.kind == ComposerMenu::Account, |p| {
                        p.width(250.).max_height(260.)
                    })
            })
            .into_iter()
            .collect();
        div()
            .relative()
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(self.composer_anchor.probe())
            .children(menu)
            .children(self.model_list_popover(cx))
            .children(self.orchestration_card(cx))
            .children(if self.is_remote() { None } else { self.usage_notice(false, cx) })
            .child(self.message_queue(window, cx))
            .children(if self.is_remote() { None } else { self.auth_notice(cx) })
            .when(self.settings.worktree_removed,|d|d.child(div().mb_2().p_3().rounded_lg().border_1().border_color(content(0.1)).flex().items_center().gap_3().text_size(px(12.)).child(div().flex_1().child("Worktree deleted. Select a branch or worktree to continue this conversation.")).child(tool_pill("reattach-worktree","Select worktree").child("Select worktree").on_click(cx.listener(|this,_,window,cx|this.open_git(GitMode::Worktree,window,cx))))))
            .when_some(self.settings.handoff.clone(), |d, handoff| {
                d.child(
                    div()
                        .mb(px(6.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(10.))
                        .bg(content(0.05))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .text_color(content(0.7))
                        .child(icon("response-handoff", 14.))
                        .child(div().min_w_0().flex_1().child(format!(
                            "Handoff from {} · Add your next instruction below",
                            Kind::display_label(&handoff.from)
                        )))
                        .child(
                            tool_pill("dismiss-handoff", "Dismiss handoff")
                                .child(icon("close", 12.))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.settings.handoff = None;
                                    this.save_settings(cx);
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.git_picker.clone(), |d, picker| {
                let anchor = if picker.read(cx).mode() == GitMode::Worktree {
                    self.workspace_anchor.clone()
                } else {
                    self.branch_anchor.clone()
                };
                let weak = cx.entity().downgrade();
                d.child(
                    popover("composer-git-picker", Anchor::Slot(anchor))
                        .side(Side::Top)
                        .width(300.)
                        .height(320.)
                        .on_dismiss(move |window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.git_picker = None;
                                this.git_events = None;
                                window.focus(&this.input.focus_handle(cx), cx);
                                cx.notify();
                            });
                        })
                        .child(picker),
                )
            })
            .child(self.approval_shelf(window, cx))
            .child(self.composer_box(window, cx))
            .children(self.remote_notice(cx))
            .child(self.composer_tools(cx))
            .into_any_element()
    }
    /// The composer box shared by sessions and Bots: its field, with send
    /// beside it, on the frosted composer surface.
    pub(crate) fn composer_box(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        const RADIUS: f32 = 8.;
        let blur = composer_blur();
        let fill = composer_fill();
        let bot = self.settings.bot.is_some();
        let input = self.input.read(cx);
        let has_value = input.has_message_text() || !self.attachments.is_empty();
        // Sessions complete files, commands and skills; a Bot's field is plain.
        let field = if bot {
            self.input.clone().into_any_element()
        } else {
            self.completion.clone().into_any_element()
        };
        div()
            .id("composer-box")
            .debug_selector(|| "composer-box".into())
            .capture_action(cx.listener(|this, _: &input::Paste, _, cx| {
                if this.paste_attachments(cx) {
                    cx.stop_propagation();
                }
            }))
            .relative()
            .rounded(px(RADIUS))
            .border_1()
            .border_color(composer_border(
                self.input.focus_handle(cx).contains_focused(window, cx),
            ))
            .when(self.file_drag, |d| {
                d.border_color(accent_color().opacity(0.6))
            })
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.input.focus_handle(cx), cx)),
            )
            .when(!bot, |d| d.child(self.completion.read(cx).composer_probe()))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let surface = gpui::fill(bounds, fill).corner_radii(px(RADIUS - 1.));
                        let surface = if blur > 0. {
                            surface.border_color(gpui_apple::metal_renderer::backdrop_blur_marker(
                                blur * window.scale_factor(),
                            ))
                        } else {
                            surface
                        };
                        window.paint_quad(surface);
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .children(self.composer_recovery_banner(cx))
            .children(self.local_submission_banner(cx))
            .children(self.attachment_chips(cx))
            .child(
                // Even padding centres send beside one line and keeps it at
                // the bottom as the text grows.
                div()
                    .flex()
                    .items_end()
                    .gap_1p5()
                    .p(px(5.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .self_center()
                            .px_1p5()
                            .py_1()
                            .child(field),
                    )
                    .child(self.action(has_value, cx)),
            )
            .children(self.attachment_drop_overlay(RADIUS))
            .into_any_element()
    }

    /// The shared `+`: files and modes for a session, files and queueing for
    /// a Bot.
    pub(crate) fn add_button(&self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.menu_is(ComposerMenu::Add);
        let queue = compact && self.settings.bot.is_some() && self.bot_ui.queue;
        quiet_control("composer-add", open || queue)
            .debug_selector(|| "composer-add".into())
            .group("composer-add")
            .w(px(26.))
            .px_0()
            .justify_center()
            .aria_label(if queue {
                "Queue is on; upload a file or change queue mode"
            } else if self.settings.bot.is_some() {
                "Upload a file or queue this message"
            } else {
                "Add files or choose a mode"
            })
            .aria_expanded(open)
            .child(self.add_anchor.probe())
            .child(
                icon("plus", 14.)
                    .text_color(content(if open || queue { 1. } else { 0.55 }))
                    .group_hover("composer-add", |s| s.text_color(content(1.))),
            )
            .on_click(
                cx.listener(|this, _, window, cx| this.toggle_menu(ComposerMenu::Add, window, cx)),
            )
    }
    /// The row under the box, shared by sessions and Bots: `+` (and for a
    /// session its working copy and branch) on the left; the model, its
    /// settings, access and context on the right.
    pub(crate) fn composer_tools(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let wide = self.composer_tools_variant(cx, false);
        let compact = self.composer_tools_variant(cx, true);
        responsive_toolbar::ResponsiveToolbar::new(wide, compact).into_any_element()
    }
    pub(super) fn composer_tools_variant(
        &mut self,
        cx: &mut Context<Self>,
        compact: bool,
    ) -> AnyElement {
        let bot = self.settings.bot.is_some();
        let kind = Kind::from_id(&self.summary.provider);
        let beside = cx
            .try_global::<vorkspace_ui::preferences::Preferences>()
            .is_some_and(|p| p.model_controls == "beside");
        let trigger_effort = (!beside).then(|| self.trigger_effort_label()).flatten();
        let controls = if beside {
            self.model_controls(cx)
        } else {
            Vec::new()
        };
        let draft = !self.summary.has_messages;
        let linked =
            self.git.as_ref().is_some_and(|s| s.linked) || self.settings.worktree_cwd.is_some();
        let new = draft && self.settings.new_worktree;
        let label = if new {
            "New worktree"
        } else if linked {
            "Worktree"
        } else {
            "Current checkout"
        };
        let enabled = !self.busy && !self.preparing_worktree && self.git.is_some();
        // Still reading git is not unavailable: don't dim, then brighten.
        let dimmed = !enabled && (self.loaded || self.busy || self.preparing_worktree);
        let workspace = if draft {
            bar_trigger(
                "composer-workspace",
                if new || linked {
                    "folder-tree"
                } else {
                    "folder"
                },
                label.into(),
            )
            .debug_selector(|| "composer-workspace".into())
            .aria_label(format!("Workspace {label}"))
            .aria_expanded(self.menu_is(ComposerMenu::Workspace))
            .child(self.workspace_anchor.probe())
            .when(dimmed, |d| d.opacity(0.4))
            .on_click(cx.listener(move |this, _, window, cx| {
                if enabled {
                    this.refresh_git(cx);
                    this.toggle_menu(ComposerMenu::Workspace, window, cx);
                }
            }))
            .into_any_element()
        } else {
            // WorkspaceIdentity: a started conversation owns this working copy.
            div()
                .id("composer-workspace")
                .debug_selector(|| "composer-workspace".into())
                .role(Role::Group)
                .aria_label(format!("Workspace {label}"))
                .flex_shrink_0()
                .h(px(26.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(content(0.55 * 0.4))
                .child(
                    icon(if linked { "folder-tree" } else { "folder" }, 14.)
                        .text_color(content(0.55 * 0.4)),
                )
                .child(label)
                .into_any_element()
        };
        let branch = if new {
            self.settings
                .worktree_base
                .as_deref()
                .map(crate::git_tools::git_picker::base_label)
                .map(str::to_owned)
                .or(self.branch.clone())
        } else {
            self.branch.clone()
        };
        if compact {
            let open = self.menu.as_ref().is_some_and(|open| {
                !matches!(open.kind, ComposerMenu::Add | ComposerMenu::Account)
            }) || self.git_picker.is_some()
                || self.bot_ui.models.is_some();
            return div()
                .debug_selector(|| "composer-toolbar".into())
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(2.))
                .pt_1()
                .pl_1()
                .pr_1p5()
                .child(self.add_button(true, cx))
                .child(
                    div()
                        .ml_auto()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            quiet_control("composer-session-settings", open)
                                .debug_selector(|| "composer-session-settings".into())
                                .flex_shrink(1.)
                                .aria_label(if bot {
                                    "Bot settings"
                                } else {
                                    "Session settings"
                                })
                                .aria_expanded(open)
                                .track_focus(&self.compact_focus)
                                .tab_index(0)
                                .child(self.compact_anchor.probe())
                                // Existing open menus follow the same visible anchor through a resize.
                                .child(self.model_anchor.probe())
                                .child(self.access_anchor.probe())
                                .child(self.workspace_anchor.probe())
                                .child(self.branch_anchor.probe())
                                .children(
                                    self.bot_ui
                                        .setting_anchors
                                        .iter()
                                        .map(|anchor| anchor.probe()),
                                )
                                .child(icon("settings", 14.))
                                .child(div().min_w_0().truncate().child(if bot {
                                    "Bot settings"
                                } else {
                                    "Session settings"
                                }))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_menu(ComposerMenu::CompactSettings, window, cx)
                                }))
                                .on_key_down(cx.listener(
                                    |this, event: &gpui::KeyDownEvent, window, cx| {
                                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                        {
                                            this.toggle_menu(
                                                ComposerMenu::CompactSettings,
                                                window,
                                                cx,
                                            );
                                            cx.stop_propagation();
                                        }
                                    },
                                )),
                        )
                        .children(self.context_meter(true, cx)),
                )
                .into_any_element();
        }
        let model_open = self.menu_is(ComposerMenu::Model)
            || self.menu_is(ComposerMenu::CompactSettings)
            || self.menu_is(ComposerMenu::Recent)
            || self.bot_ui.models.is_some();
        let model = quiet_control("composer-model", model_open)
            .debug_selector(|| "composer-model".into())
            .min_w_0()
            .max_w(px(220.))
            .flex_shrink(1.)
            .aria_label(format!(
                "{}, {}{}",
                kind.label(),
                self.model_label(),
                trigger_effort
                    .as_ref()
                    .map(|effort| format!(", effort {effort}"))
                    .unwrap_or_default()
            ))
            .child(self.model_anchor.probe())
            .child(self.compact_anchor.probe())
            .track_focus(&self.compact_focus)
            .tab_index(0)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    if this.menu_is(ComposerMenu::CompactSettings) {
                        this.close_menu(window, cx);
                    } else {
                        this.toggle_menu(ComposerMenu::Model, window, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .aria_expanded(model_open)
            .on_click(cx.listener(|this, _, window, cx| {
                if this.menu_is(ComposerMenu::CompactSettings) {
                    this.close_menu(window, cx);
                } else {
                    this.toggle_menu(ComposerMenu::Model, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_recent_models(window, cx)
                }),
            )
            .child(provider_icon(&self.summary.provider, 14.))
            .child(
                div()
                    .debug_selector(|| "composer-model-label".into())
                    .min_w_0()
                    .truncate()
                    .child(self.model_label()),
            )
            .when_some(trigger_effort, |d, effort| {
                d.child(
                    div()
                        .debug_selector(|| "composer-model-effort".into())
                        .min_w_0()
                        .flex_shrink(1.)
                        .truncate()
                        .text_color(content(0.5))
                        .child(effort),
                )
            });
        let access_open = self.menu_is(ComposerMenu::Access);
        let access = quiet_control("composer-access", access_open)
            .debug_selector(|| "composer-access".into())
            .aria_label(format!("Access, {}", self.settings.access.label()))
            .aria_expanded(access_open)
            .child(self.access_anchor.probe())
            .on_click(
                cx.listener(|this, _, window, cx| {
                    this.toggle_menu(ComposerMenu::Access, window, cx)
                }),
            )
            .child(
                icon(
                    match self.settings.access {
                        AccessMode::FullAccess => "shield-alert",
                        AccessMode::AutoAcceptEdits => "pencil-edit",
                        AccessMode::Auto => "sparkles",
                        _ => "square-lock",
                    },
                    14.,
                )
                .text_color(if self.settings.access == AccessMode::FullAccess {
                    status(StatusColor::Warning, 1.)
                } else {
                    content(if access_open { 1. } else { 0.55 })
                }),
            )
            .child(div().truncate().child(self.settings.access.label()));
        div()
            .debug_selector(|| "composer-toolbar".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(2.))
            .pt_1()
            .pl_1()
            .pr_1p5()
            .child(self.add_button(false, cx))
            .when(bot && self.bot_ui.queue, |d| {
                d.child(
                    quiet_control("composer-queue-mode", true)
                        .debug_selector(|| "composer-queue-mode".into())
                        .aria_label("Queue is on; click to send normally")
                        .child(icon("queue-list", 14.))
                        .child("Queue")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.bot_ui.queue = false;
                            cx.notify();
                        })),
                )
            })
            // A Bot works across its projects; a session owns one checkout.
            .when(!bot, |d| d.child(workspace))
            .when_some(branch.filter(|_| !bot), |d, branch| {
                let detached = !new && self.git.as_ref().is_some_and(|s| s.detached);
                let label = if detached {
                    format!("detached {branch}")
                } else {
                    branch
                };
                d.child(
                    bar_trigger("composer-branch", "git-branch", label.clone().into())
                        .debug_selector(|| "composer-branch".into())
                        .aria_label(format!(
                            "{} {label}",
                            if new { "Worktree base" } else { "Branch" }
                        ))
                        .aria_description(self.execution_cwd().to_owned())
                        .max_w(px(256.))
                        .child(self.branch_anchor.probe())
                        .when(dimmed, |d| d.opacity(0.4))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if enabled {
                                this.open_git(
                                    if new { GitMode::Base } else { GitMode::Branch },
                                    window,
                                    cx,
                                );
                            }
                        })),
                )
            })
            .child(
                div()
                    .ml_auto()
                    .pl_2()
                    .min_w_0()
                    .max_w_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(model)
                    .children(controls)
                    .when(!bot, |d| d.child(access))
                    .when(self.editing_last_turn.pending(), |d| {
                        d.child(
                            div()
                                .id("cancel-edit-last-turn")
                                .role(Role::Button)
                                .aria_label("Cancel editing")
                                .text_size(px(11.))
                                .text_color(content(0.6))
                                .child("Cancel edit")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel_edit_last_turn(cx);
                                })),
                        )
                    })
                    .when(
                        !bot && self.status != "Ready"
                            && !self.busy
                            && !self.loading
                            && self.settings.usage_limit.is_none(),
                        |d| {
                            d.child(
                                div()
                                    .id("provider-status")
                                    .debug_selector(|| "provider-status".into())
                                    .role(Role::Status)
                                    .aria_label(self.status.clone())
                                    .max_w(px(220.))
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(content(0.45))
                                    .child(self.status.clone()),
                            )
                        },
                    )
                    .children(self.context_meter(false, cx)),
            )
            .into_any_element()
    }

    pub(super) fn context_meter(&self, force: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bot = self.settings.bot.is_some();
        let draft = !self.summary.has_messages;
        let ratio = self.context.and_then(|usage| usage.ratio());
        let show_context = force || bot || ratio.is_some() || draft;
        let headline = ratio
            .map(|r| format!("{:.0}% context used", r * 100.))
            .unwrap_or_else(|| {
                if draft && self.context.is_none() {
                    "No context used yet"
                } else {
                    "Context usage unavailable"
                }
                .into()
            });
        let detail = self.context.map(|usage| {
            let used = vorkspace_core::context::format_tokens(usage.used);
            match usage.window.filter(|window| *window > 0) {
                Some(window) => format!(
                    "{used} / {} tokens",
                    vorkspace_core::context::format_tokens(window)
                ),
                None => format!("{used} tokens used; context window unknown"),
            }
        });
        show_context.then(|| {
            div()
                .id("context-meter")
                .debug_selector(|| "context-meter".into())
                .role(Role::Image)
                .aria_label(format!(
                    "{headline}{}",
                    detail
                        .as_ref()
                        .map(|s| format!(", {s}"))
                        .unwrap_or_default()
                ))
                .child(self.context_anchor.probe())
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.context_hovered = *hovered;
                    cx.notify();
                }))
                .child(vorkspace_ui::context_meter::ring(ratio.unwrap_or(0.)))
                .when(self.context_hovered, |d| {
                    d.child(
                        popover("context-tooltip", Anchor::Slot(self.context_anchor.clone()))
                            .side(Side::Top)
                            .align(Align::End)
                            .max_width(240.)
                            .padding(10.)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .text_color(content(1.))
                                    .child(headline)
                                    .when_some(detail, |d, detail| {
                                        d.child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(content(0.5))
                                                .child(detail),
                                        )
                                    }),
                            ),
                    )
                })
                .into_any_element()
        })
    }

    // ComposerAction: a solid submit, dimmed while empty; Stop while busy.
    pub(super) fn action(&self, has_value: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let ticket_pending = self.ticket_context.pending();
        let bot = self.settings.bot.is_some();
        let busy = !bot && (ticket_pending || (self.responding() && !has_value));
        let draft = self.draft_active(cx);
        let enabled = ticket_pending
            || (!self.attaching
                && (!self.preparing_worktree || self.busy || self.stopping)
                && (busy || (has_value && self.loaded))
                && (busy
                    || !draft
                    || (self.follow_ups.state.draft.is_none()
                        && (!self.is_remote() || (!self.busy && !self.stopping)))));
        let accent = primary_action(enabled);
        vorkspace_ui::theme::keyboard_reachable(div().id("send-or-stop"))
            .debug_selector(|| "send-or-stop".into())
            .role(Role::Button)
            .aria_label(if busy {
                "Stop"
            } else if draft {
                "Save draft"
            } else {
                "Send"
            })
            .cursor(CursorStyle::Arrow)
            .flex_shrink_0()
            .size(px(26.))
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match accent {
                Some((fill, _)) => d
                    .bg(fill)
                    .when(enabled, |d| d.hover(|s| s.bg(fill.blend(content(0.12))))),
                None => d
                    .bg(white(if enabled { 1. } else { 0.3 }))
                    .when(enabled, |d| d.hover(|s| s.bg(white(0.9)))),
            })
            .map(|d| {
                let ink = accent.map_or(black(if enabled { 1. } else { 0.4 }), |(_, ink)| ink);
                if busy {
                    d.child(div().size(px(10.)).rounded(px(2.)).bg(ink))
                } else {
                    d.child(icon("send", 14.).text_color(ink))
                }
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                if !enabled {
                    return;
                }
                if busy {
                    this.stop(cx);
                } else {
                    this.send(cx);
                }
            }))
    }
    // EmptySession.tsx: the composer centred under its question, or resting
    // where it will dock once a neighbouring pane squeezes it narrower than
    // the column. A narrow window alone keeps it centred.
    pub(super) fn empty(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let narrow = self.narrow;
        let docked = narrow && self.beside_pane;
        self.attachment_drop_target("empty-session", cx)
            .relative()
            .size_full()
            .flex()
            .justify_center()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let now = bounds.size.width < px(COMPOSER_COLUMN);
                        if now != narrow {
                            cx.defer(move |cx| {
                                let _ = this.update(cx, |this, cx| {
                                    this.narrow = now;
                                    cx.notify();
                                });
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(COMPOSER_COLUMN))
                    .flex()
                    .flex_col()
                    .px(px(10.))
                    .pt(px(24.))
                    .pb(px(6.))
                    .child(div().flex_grow(1.).flex_basis(px(0.)))
                    .when(!docked, |d| {
                        d.child(
                            div()
                                .id("empty-session-title")
                                .role(Role::Heading)
                                .flex_shrink_0()
                                .mb_4()
                                .px(px(10.))
                                .text_size(px(18.))
                                .line_height(px(28.))
                                .text_color(content(1.))
                                .truncate()
                                .child(format!(
                                    "What should we work on in {}?",
                                    self.project_appearance.label(&self.summary.cwd)
                                )),
                        )
                    })
                    .child(self.composer(window, cx))
                    .when(!docked, |d| {
                        d.child(div().flex_grow(1.).flex_shrink(1.).flex_basis(px(18.)))
                    }),
            )
            .into_any()
    }
}
