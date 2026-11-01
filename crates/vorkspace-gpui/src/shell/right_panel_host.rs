//! The shell's ownership of the right tool panel: shown state, framing and resizing.
use super::*;

impl Shell {
    /// App.tsx's `insetRightPanelShown`: open (or sliding shut), over a
    /// project, and not covered by an app view.
    pub(super) fn right_panel_shown(&self, cx: &App) -> bool {
        self.bot_details_active(cx)
            && (self.workspace.right_panel_open || self.right_slide.is_some())
            && !self.app_view_open()
            && self.settings.is_none()
            && self.app_search.is_none()
            && (self.bot_active(cx) || self.git.read(cx).available())
    }
    pub(super) fn sync_right_panel(&mut self, cx: &mut Context<Self>) {
        let opening = self.right_slide.as_ref().is_some_and(PaneSlide::opening);
        let classic_tools = self.workspace.classic_layout
            && (self.workspace.sidebar_visible || self.peek_open())
            && self.workspace.sidebar_tab() != crate::workspace::SidebarTab::Sessions;
        let shown = !self.bot_active(cx)
            && (classic_tools
                || (!self.workspace.classic_layout && self.workspace.right_panel_open && !opening))
            && !self.app_view_open()
            && self.settings.is_none()
            && self.app_search.is_none();
        let tab = if self.workspace.classic_layout {
            self.classic_tool_tab()
        } else {
            Some(self.workspace.right_panel_tab)
        };
        if let Some(panel) = &self.right_panel {
            panel.update(cx, |p, cx| {
                if let Some(tab) = tab {
                    p.restore_tab(tab, cx);
                }
                p.set_shown(shown, cx);
            });
        }
    }
    pub(super) fn toggle_right_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bot_active(cx) && !self.bot_details_active(cx) {
            return;
        }
        if self.workspace.classic_layout && !self.bot_active(cx) {
            self.toggle_sidebar(window, cx);
            return;
        }
        self.set_right_panel_open(!self.workspace.right_panel_open, window, cx);
    }
    pub(super) fn set_right_panel_open(
        &mut self,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.bot_active(cx) && !self.bot_details_active(cx) {
            return;
        }
        if self.workspace.classic_layout && !self.bot_active(cx) {
            let tab = if open {
                self.right_panel
                    .as_ref()
                    .map(|p| Self::classic_tab_for_tool(p.read(cx).tab()))
                    .unwrap_or(crate::workspace::SidebarTab::Files)
            } else {
                crate::workspace::SidebarTab::Sessions
            };
            self.select_classic_sidebar(tab, window, cx);
            return;
        }
        if self.app_view_open()
            || (!self.bot_active(cx) && !self.git.read(cx).available())
            || self.workspace.right_panel_open == open
        {
            return;
        }
        let details_focus = if let Some(bots) = self
            .bots
            .as_ref()
            .filter(|m| m.read(cx).active.is_some() || m.read(cx).classic_channels_open())
        {
            Some(bots.read(cx).details.focus_handle(cx))
        } else {
            self.right_panel.as_ref().map(|p| p.focus_handle(cx))
        };
        if !open
            && details_focus
                .as_ref()
                .is_some_and(|focus| focus.contains_focused(window, cx))
        {
            self.focus_active(window, cx);
        }
        let full = self.workspace.right_panel_width;
        let to = if open { full } else { 0. };
        self.workspace.right_panel_open = open;
        let previous = self.right_slide.take();
        self.right_slide = Some(match previous {
            Some(slide) => slide.retarget(to, vorkspace_ui::clock::Instant::now()),
            None => PaneSlide::new(if open { 0. } else { full }, to, full),
        });
        self.sync_right_panel(cx);
        if open
            && !self.bot_active(cx)
            && let Some(panel) = self.right_panel.clone()
        {
            panel.update(cx, |p, cx| p.focus_pane(window, cx));
        }
        self.fit_sidebar(cx);
        self.persist(cx);
        cx.notify();
    }
    pub(super) fn ensure_right_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.right_panel.is_some() {
            return;
        }
        let git = self.git.clone();
        let (tab, order, view) = (
            self.workspace.right_panel_tab,
            self.workspace.right_panel_tabs.clone(),
            self.workspace.changes_view,
        );
        let panel = cx.new(|cx| RightPanel::new(git, tab, order, window, cx));
        let explorer = panel.read(cx).explorer.clone();
        self.observations.push(cx.subscribe(
            &explorer,
            |this, _, event: &crate::file_tools::explorer::FileActionError, cx| {
                this.notification_state
                    .file_error
                    .update(cx, |notice, cx| notice.set_message(event.0.clone(), cx));
                cx.notify();
            },
        ));
        self.observations.push(cx.subscribe_in(
            &explorer,
            window,
            |this, _, event: &crate::file_tools::explorer::OpenFile, window, cx| {
                this.open_review(
                    crate::git_tools::review::ReviewTab::File {
                        cwd: this.git.read(cx).project().into(),
                        path: event.0.clone(),
                    },
                    window,
                    cx,
                );
            },
        ));
        self.observations.push(cx.subscribe_in(
            &explorer,
            window,
            |this, _, event: &crate::file_tools::explorer::OpenSearchMatch, window, cx| {
                this.open_review_with_preview(
                    crate::git_tools::review::ReviewTab::File {
                        cwd: event.cwd.clone(),
                        path: event.matched.path.clone(),
                    },
                    Some(event.pin),
                    window,
                    cx,
                );
                if let Some(pane) = this.active_review(cx) {
                    pane.update(cx, |pane, cx| {
                        pane.navigate_file(
                            event.matched.line as usize,
                            event.matched.column as usize,
                            cx,
                        )
                    });
                }
            },
        ));
        let changes = panel.read(cx).changes.clone();
        changes.update(cx, |c, cx| c.set_view(view, cx));
        self.observations
            .push(cx.subscribe(&panel, |this, _, event: &PanelEvent, cx| {
                match event {
                    PanelEvent::Tab(tab) => {
                        if this.workspace.classic_layout {
                            this.workspace.sidebar_tabs.insert(
                                this.workspace.project.clone(),
                                Self::classic_tab_for_tool(*tab),
                            );
                        } else {
                            this.workspace.right_panel_tab = *tab;
                        }
                        this.invalidate_chrome(cx);
                        cx.notify();
                    }
                    PanelEvent::Reordered(order) => this.workspace.right_panel_tabs = order.clone(),
                }
                this.persist(cx);
            }));
        self.observations.push(cx.subscribe_in(
            &changes,
            window,
            |this, _, event: &crate::git_tools::changes::ChangesEvent, window, cx| match event {
                crate::git_tools::changes::ChangesEvent::View(view) => {
                    this.workspace.changes_view = *view;
                    this.persist(cx);
                }
                crate::git_tools::changes::ChangesEvent::OpenDiff { cwd, path, staged } => {
                    this.open_review(
                        crate::git_tools::review::ReviewTab::Changes {
                            cwd: cwd.clone(),
                            path: path.clone(),
                            staged: Some(*staged),
                        },
                        window,
                        cx,
                    );
                }
            },
        ));
        let history = panel.read(cx).history.clone();
        self.observations.push(cx.subscribe_in(
            &history,
            window,
            |this, _, event: &crate::git_tools::history::HistoryEvent, window, cx| {
                let crate::git_tools::history::HistoryEvent::OpenCommit {
                    cwd,
                    sha,
                    short_sha,
                    subject,
                } = event;
                this.open_review(
                    crate::git_tools::review::ReviewTab::Commit {
                        cwd: cwd.clone(),
                        sha: sha.clone(),
                        short_sha: short_sha.clone(),
                        subject: subject.clone(),
                    },
                    window,
                    cx,
                );
            },
        ));
        self.right_panel = Some(panel);
        self.sync_git(cx);
    }
    /// Part of the card: the project tools beside the workspace, behind a
    /// hairline. Its contents keep their full width while it slides.
    pub(super) fn right_panel_frame(
        &self,
        panel: Entity<RightPanel>,
        width: f32,
        full: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = if let Some(bots) = self
            .bots
            .as_ref()
            .filter(|m| m.read(cx).active.is_some() || m.read(cx).classic_channels_open())
        {
            let manager = bots.read(cx);
            let details = manager.details.clone();
            let html = manager.native_detail_content(cx);
            // Native artifact views must update their frames and clipping
            // on every paint; replaying cached GPU quads skips that work.
            if html {
                details.into_any_element()
            } else {
                details
                    .cached(StyleRefinement::default().size_full())
                    .into_any_element()
            }
        } else {
            panel
                .cached(StyleRefinement::default().size_full())
                .into_any_element()
        };
        div()
            .id("right-panel-frame")
            .debug_selector(|| "right-panel-frame".into())
            .relative()
            .h_full()
            .flex_shrink_0()
            .w(px(width))
            .border_l_1()
            .border_color(crate::panel_kit::stroke())
            // The full-width contents travel with the leading edge instead of
            // reflowing into the growing clip on every animation frame.
            .child(
                div().relative().size_full().overflow_hidden().child(
                    div()
                        .id("right-panel-content")
                        .debug_selector(|| "right-panel-content".into())
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(px(full - 1.))
                        .child(content),
                ),
            )
            .child(
                self.right_panel_resize_handle("right-panel-resize", cx)
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-3.5))
                    .child(resize_border(true, self.right_resize.is_some())),
            )
            .into_any_element()
    }

    pub(super) fn right_panel_resize_handle(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .group("resize-handle")
            .role(Role::Splitter)
            .aria_label("Resize right sidebar")
            .w(px(6.))
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.right_resize = Some((event.position.x, this.workspace.right_panel_width));
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_click(cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.right_resize = None;
                    this.workspace.right_panel_width = crate::right_panel::WIDTH_DEFAULT;
                    this.persist(cx);
                    cx.notify();
                }
            }))
    }
    /// Bot detail layers occlude the shell's ordinary mouse handlers. An
    /// active resize must keep receiving moves and release over those layers.
    pub(super) fn right_panel_resize_watch(&self, cx: &Context<Self>) -> AnyElement {
        let shell = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let moves = shell.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = moves.update(cx, |this, cx| {
                            if this.drag_right_panel(event, cx) {
                                cx.stop_propagation();
                            }
                        });
                    }
                });
                let release = shell.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        let _ = release.update(cx, |this, cx| {
                            if this.right_resize.is_some() {
                                this.release_resizes(cx);
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .size_0()
        .into_any_element()
    }

    pub(super) fn drag_right_panel(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((start, width)) = self.right_resize else {
            return false;
        };
        if event.pressed_button == Some(MouseButton::Left) {
            let width = width - (event.position.x - start).as_f32();
            self.drag_resize(ResizeTarget::RightPanel, width, cx);
        } else {
            self.release_resizes(cx);
        }
        true
    }
}
