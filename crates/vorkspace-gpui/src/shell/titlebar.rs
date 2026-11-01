//! Titlebar layout: navigation controls, project tools and their reserved widths.
use super::*;

impl Shell {
    /// The title bar's navigation (`data-inset-title-nav`): as wide as the
    /// open sidebar, with history at its far edge; with the sidebar closed,
    /// just the traffic lights' gap and its three buttons.
    pub(super) fn traffic_lights(&self, window: &Window) -> f32 {
        traffic_light_inset(window.ui_scale(), self.fullscreen)
    }
    pub(super) fn navigation_padding(&self) -> f32 {
        if self.workspace.project_rail_open {
            6.
        } else {
            0.
        }
    }
    pub(super) fn navigation_button_size(window: &Window) -> f32 {
        // All three hit areas must occupy whole device pixels. Independently
        // rounding 24 logical units can otherwise overflow their combined budget
        // at fractional zoom (for example, three 22px buttons at 90% on 1x).
        let scale = window.scale_factor();
        (24. * scale).round() / scale
    }
    pub(super) fn navigation_width(&self, window: &Window) -> f32 {
        let scale = window.scale_factor();
        let minimum = self.traffic_lights(window)
            + 3. * Self::navigation_button_size(window)
            + self.navigation_padding();
        let minimum = (minimum * scale).ceil() / scale;
        self.classic_rail_width().max(minimum)
    }
    /// Room before the tabs where navigation overhangs the pane.
    pub(super) fn strip_lead(&self, window: &Window) -> f32 {
        // A left sidebar takes the overhang in its own row (`classic_body`).
        let sidebar = if self.workspace.preferences.sidebar_right {
            0.
        } else {
            self.workspace.right_panel_width * self.sidebar_fraction
        };
        (self.rail_overhang(window) - sidebar).max(0.)
    }
    /// How far navigation reaches past the project rail.
    pub(super) fn rail_overhang(&self, window: &Window) -> f32 {
        (self.navigation_width(window) - self.classic_rail_width()).max(0.)
    }
    /// The traffic lights, sidebar toggle and visit arrows, spanning the
    /// open sidebar so the arrows sit at its edge.
    pub(super) fn navigation(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let toggle_focus = self.project_rail_toggle_focus.clone();
        let button_size = px(Self::navigation_button_size(window));
        div()
            .debug_selector(|| "titlebar-navigation".into())
            .w(px(self.navigation_width(window)))
            .h(px(TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            // Keep history aligned with the project rail in both workspace modes.
            .pr(px(self.navigation_padding()))
            .child(
                // Fixed, so the toggle holds still as the sidebar opens
                // and closes.
                titlebar_drag_area()
                    .w(px(self.traffic_lights(window)))
                    .flex_shrink_0(),
            )
            .child(
                icon_button("toggle-sidebar", TOGGLE_SIDEBAR_LABEL, "sidebar")
                    .debug_selector(|| "toggle-sidebar".into())
                    .size(button_size)
                    .track_focus(&self.project_rail_toggle_focus)
                    .focus(|s| s.bg(content(0.1)))
                    .child(self.peek_toggle_bounds.probe())
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&toggle_focus, cx);
                        cx.stop_propagation();
                    })
                    // Hovering the toggle peeks a collapsed sidebar out.
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_project_rail(cx))),
            )
            .child(titlebar_drag_area().flex_1())
            .child(
                icon_button_enabled(
                    "visit-back",
                    BACK_LABEL,
                    "chevron-left",
                    !self.history.back.is_empty(),
                )
                .debug_selector(|| "visit-back".into())
                .size(button_size)
                .on_click(cx.listener(|this, _, window, cx| this.visit(-1, window, cx))),
            )
            .child(
                icon_button_enabled(
                    "visit-forward",
                    FORWARD_LABEL,
                    "chevron-right",
                    !self.history.forward.is_empty(),
                )
                .debug_selector(|| "visit-forward".into())
                .size(button_size)
                .on_click(cx.listener(|this, _, window, cx| this.visit(1, window, cx))),
            )
    }
    pub(super) fn titlebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Same hierarchy as TitleBar.tsx's inset layout: window navigation and
        // session tabs share the outer shell, above the rounded workspace card.
        if let Some(settings) = self.settings.clone() {
            let classic = self.workspace.classic_layout;
            let lead = if classic {
                self.strip_lead(window)
            } else {
                self.navigation_width(window) + 6.
            };
            let header = settings.update(cx, |v, cx| v.header(!classic, cx));
            return div()
                .h(px(TITLE_BAR_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .when(classic, |d| d.border_b_1().border_color(content(0.1)))
                .child(if classic {
                    // Covered by floating navigation, which owns gestures here.
                    div()
                        .h_full()
                        .w(px(lead))
                        .flex_shrink_0()
                        .into_any_element()
                } else {
                    self.navigation(window, cx).into_any_element()
                })
                .child(header)
                .into_any_element();
        }
        // Vox has no title bar control for now.
        let tabs_width = px(self.tabs_width(window, cx).max(0.));
        let classic = self.workspace.classic_layout;
        div()
            .h(px(TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            // Classic: the strip heads the workspace column, above a rule. The tab
            // strip draws its own stretch of the rule, around the active tab's
            // pill (crate::liquid); the lead and trail draw theirs.
            .child(if classic {
                // Reserve the navigation's overhang without a second drag
                // handler underneath its buttons (classic_body owns gestures).
                // Where the rail's divider curves into it, the rule starts
                // after the curve.
                let curve = if self.rail_meets_title_rule()
                    && (self.workspace.preferences.sidebar_right || self.sidebar_fraction <= 0.)
                {
                    crate::liquid::PANE_RADIUS
                } else {
                    0.
                };
                div()
                    .h_full()
                    .w(px(self.strip_lead(window)))
                    .flex_shrink_0()
                    .pl(px(curve))
                    .child(
                        div()
                            .size_full()
                            .border_b_1()
                            .border_color(crate::panel_kit::stroke()),
                    )
                    .into_any_element()
            } else {
                self.navigation(window, cx).into_any_element()
            })
            .child(if self.tabless_page(cx) {
                titlebar_drag_area()
                    .w(tabs_width)
                    .h_full()
                    .flex_shrink_0()
                    .when(classic, |d| {
                        d.border_b_1().border_color(crate::panel_kit::stroke())
                    })
                    .into_any_element()
            } else {
                self.tabs_frame
                    .clone()
                    .cached(
                        StyleRefinement::default()
                            .w(tabs_width)
                            .h(px(TITLE_BAR_HEIGHT)),
                    )
                    .into_any_element()
            })
            .child(if classic {
                // The toggle holds the window's corner (classic_body); the
                // strip keeps only the part an open panel leaves uncovered.
                titlebar_drag_area()
                    .w(px(self.strip_trail(cx)))
                    .h_full()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(crate::panel_kit::stroke())
                    .into_any_element()
            } else {
                self.panel_toggle(window, cx)
            })
            .into_any()
    }

    /// TitleBar.tsx's right panel toggle, `pl-1.5 pr-3` clear of the edge.
    pub(super) fn panel_toggle(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let bot = self.bot_details_active(cx);
        if self.workspace.classic_layout && !bot {
            return self.classic_corner(window, cx);
        }
        let toggle = bot || self.git.read(cx).available();
        let panel_off = self.app_view_open();
        let dot = !bot && !panel_off && self.git.read(cx).has_changes();
        let panel_active = self.workspace.right_panel_open && !panel_off;
        let tabs = if bot && panel_active {
            self.bots.clone().map(|m| {
                m.update(cx, |m, cx| {
                    m.overview_controls(self.workspace.right_panel_width >= 312., window, cx)
                })
            })
        } else {
            self.right_panel
                .as_ref()
                .filter(|_| panel_active && !bot)
                .map(|panel| {
                    panel
                        .read(cx)
                        .tabs
                        .clone()
                        .cached(
                            StyleRefinement::default()
                                .w(px(self.project_tabs_width(cx)))
                                .h(px(TITLE_BAR_HEIGHT)),
                        )
                        .into_any_element()
                })
        };
        let show_tabs = tabs.is_some();
        div()
            .relative()
            .w(px(self.toggle_room(cx)))
            .h(px(TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .map(|d| {
                if !toggle {
                    return d.pl_1p5().pr_3().child(self.file_search_button(cx));
                }
                let ink = if panel_off {
                    content(0.25)
                } else if panel_active && !self.workspace.classic_layout {
                    content(1.)
                } else {
                    content(0.5)
                };
                d.child(
                    div()
                        .h_full()
                        .w_full()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .pl(px(if show_tabs && !self.workspace.classic_layout {
                            CORNER_TABS_LEAD
                        } else {
                            6.
                        }))
                        .pr_3()
                        .when_some(tabs, |d, tabs| {
                            d.child(tabs)
                                .when(!bot, |d| d.child(titlebar_drag_area().flex_1().min_w_0()))
                        })
                        .children(self.project_diff_stat(cx))
                        // A Bot has no project files to search; its toggle keeps the corner.
                        .map(|d| {
                            if bot {
                                d.child(div().flex_1())
                            } else {
                                d.child(self.file_search_button(cx).mx(px(CORNER_CONTROL_GAP)))
                            }
                        })
                        .child(
                            div()
                                .id("toggle-right-panel")
                                .debug_selector(|| "toggle-right-panel".into())
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .role(Role::Button)
                                .aria_label(if dot {
                                    TOGGLE_RIGHT_PANEL_DOT_LABEL
                                } else {
                                    TOGGLE_RIGHT_PANEL_LABEL
                                })
                                .aria_toggled(if panel_active {
                                    gpui::Toggled::True
                                } else {
                                    gpui::Toggled::False
                                })
                                .group("toggle-right-panel")
                                .cursor(CursorStyle::Arrow)
                                .size(px(24.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .when(!panel_off, |d| d.hover(|s| s.bg(content(0.1))))
                                .child(
                                    div()
                                        .relative()
                                        .size(px(14.))
                                        .child(icon("sidebar-right", 14.).text_color(ink).when(
                                            !panel_off,
                                            |i| {
                                                i.group_hover("toggle-right-panel", |s| {
                                                    s.text_color(content(1.))
                                                })
                                            },
                                        ))
                                        .when(dot, |d| {
                                            d.child(
                                                div()
                                                    .absolute()
                                                    .top(px(-2.))
                                                    .right(px(-2.))
                                                    .size(px(6.))
                                                    .rounded_full()
                                                    .bg(accent_color()),
                                            )
                                        }),
                                )
                                .when(!panel_off, |d| {
                                    d.on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_right_panel(window, cx)
                                    }))
                                }),
                        ),
                )
            })
            .when(show_tabs && !self.workspace.classic_layout, |d| {
                // The card has 6px of outer padding and a 1px right border.
                // Keep the rule on its divider even during the opening slide.
                let offset = self.toggle_room(cx) - (self.right_drawn + 7.);
                d.child(
                    self.right_panel_resize_handle("titlebar-right-panel-resize", cx)
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(offset - 2.5))
                        .child(
                            div()
                                .id("project-tabs-separator")
                                .debug_selector(|| "project-tabs-separator".into())
                                .absolute()
                                .left(px(2.5))
                                .top(px((TITLE_BAR_HEIGHT - 16.) / 2.))
                                .w(px(1.))
                                .h(px(16.))
                                .bg(crate::panel_kit::stroke())
                                .when(self.right_resize.is_some(), |d| d.bg(content(0.16)))
                                .group_hover("resize-handle", |s| s.bg(content(0.16))),
                        ),
                )
            })
            .into_any()
    }
    pub(super) fn project_diff_stat(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let width = self.diff_stat_room(cx);
        (width > 0.).then(|| {
            crate::panel_kit::diff_stat(self.git.read(cx).stats.as_ref(), width)
                .when(!self.app_view_open(), |d| {
                    d.cursor(CursorStyle::Arrow)
                        .hover(|s| s.bg(content(0.1)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.ensure_right_panel(window, cx);
                            let panel = this.right_panel.clone().unwrap();
                            let previous = panel.read(cx).tab();
                            let changes = crate::panel_kit::RightTab::Changes;
                            if previous != changes {
                                this.corner_reveal =
                                    Some(crate::project_tabs::TabReveal::new(changes, previous));
                            }
                            panel.update(cx, |panel, cx| panel.select(changes, cx));
                            this.set_right_panel_open(true, window, cx);
                            panel.update(cx, |panel, cx| panel.focus_pane(window, cx));
                            this.invalidate_chrome(cx);
                            cx.notify();
                        }))
                })
                .into_any_element()
        })
    }
    pub(super) fn file_search_button(&self, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let label = "Search files (⌘P)";
        icon_button("titlebar-file-search", label, "search")
            .debug_selector(|| "titlebar-file-search".into())
            .flex_shrink_0()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| this.open_quick_open(false, window, cx)))
    }
    pub(super) fn diff_stat_room(&self, cx: &App) -> f32 {
        if self.bot_active(cx) || !self.git.read(cx).available() {
            return 0.;
        }
        let natural = crate::panel_kit::diff_stat_width(self.git.read(cx).stats.as_ref());
        if !self.workspace.right_panel_open || self.app_view_open() {
            return natural;
        }
        let controls = crate::right_panel::TAB_ICONS_WIDTH + 6. + CORNER_BUTTONS_WIDTH + 12.;
        natural.min((self.workspace.right_panel_width - controls).max(48.))
    }
    pub(super) fn project_tabs_width(&self, cx: &App) -> f32 {
        let padding = if self.workspace.classic_layout {
            24.
        } else {
            12.
        };
        (self.workspace.right_panel_width
            - self.diff_stat_room(cx)
            - CORNER_BUTTONS_WIDTH
            - padding)
            .clamp(
                crate::right_panel::TAB_ICONS_WIDTH,
                crate::right_panel::TABS_WIDTH,
            )
    }
    /// The toggle's room at the end of the strip: `pl-1.5`, the button, `pr-3`.
    pub(super) fn toggle_room(&self, cx: &App) -> f32 {
        if self.workspace.classic_layout {
            return self.corner_room(cx);
        }
        if self.bot_active(cx) || self.git.read(cx).available() {
            if self.workspace.right_panel_open && !self.app_view_open() {
                if self.bot_active(cx) {
                    let minimum = if self.workspace.right_panel_width >= 312. {
                        312.
                    } else {
                        252.
                    };
                    return (self.right_drawn + 7.).max(minimum);
                }
                // Follow the rounded panel's left edge, including the card's
                // right inset, while keeping room for all controls when narrow.
                (self.right_drawn + 7.).max(
                    CORNER_TABS_LEAD
                        + self.project_tabs_width(cx)
                        + self.diff_stat_room(cx)
                        + CORNER_BUTTONS_WIDTH
                        + 12.,
                )
            } else {
                6. + self.diff_stat_room(cx) + CORNER_BUTTONS_WIDTH + 12.
            }
        } else {
            6. + 24. + 12.
        }
    }
    /// Classic: the strip's room after its tabs, wherever the toggle
    /// overhangs an open right panel.
    pub(super) fn strip_trail(&self, cx: &App) -> f32 {
        (self.toggle_room(cx) - self.right_drawn).max(0.)
    }
}
