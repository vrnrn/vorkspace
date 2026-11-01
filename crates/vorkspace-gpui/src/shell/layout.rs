// Project rail, conversation workspace, then project tools on the right.
// Each column owns its header; subscriptions live above the rail settings.
use super::{CORNER_BUTTONS_WIDTH, Shell};
use crate::{
    popover::{Anchor, popover},
    resource_pressure::SessionInfo,
    right_panel::RightPanel,
    theme::*,
};
use gpui::{
    AnyElement, Context, CursorStyle, Entity, MouseButton, MouseDownEvent, StyleRefinement, Window,
    div, prelude::*, px,
};
use std::collections::HashMap;

impl Shell {
    pub(super) fn layout_body(
        &mut self,
        sidebar_width: f32,
        right_shown: bool,
        right_width: f32,
        full: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.resources
            .update(cx, |resources, cx| resources.set_classic(true, cx));
        vec![self.classic_body(sidebar_width, right_shown, right_width, full, window, cx)]
    }
    fn classic_body(
        &mut self,
        sidebar_width: f32,
        right_shown: bool,
        right_width: f32,
        full: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card = self.workspace_content(false, right_width, full, window, cx);
        // Project tools and Bot details occupy the same right-hand edge and
        // share a saved width, while retaining their own visibility and content.
        let panel = self
            .right_panel
            .clone()
            .filter(|_| right_shown && self.settings.is_none())
            .map(|panel| self.right_panel_frame(panel, right_width, full, cx));
        // Search and app views (connectors, Notes, Automations) start at the top edge.
        // Settings keeps its own header, whichever page it was opened from.
        let app_view = self.settings.is_none()
            && (self.active_app_view.is_some() || self.app_search.is_some());
        let bare = self.bare_page(cx);
        // Collapsed navigation can extend past the rail. Keep its drag row
        // above page headers so the controls cannot cover their inputs.
        let tab_bar = (!bare || self.strip_lead(window) > 0.).then(|| self.titlebar(window, cx));
        let navigation = self.navigation(window, cx);
        let rail_width = self.classic_rail_width();
        // At zero width the cached rail is not rendered, so it cannot stop
        // presentation tasks itself.
        if rail_width <= 0. {
            self.hide_project_marks(cx);
        }
        let corner = (self.settings.is_none() && !app_view).then(|| self.panel_toggle(window, cx));
        let header = self.classic_workspace_header(cx);
        // Sessions, Explorer and Changes sit beside the rail or at the right edge.
        let left = !self.workspace.preferences.sidebar_right;
        let sidebar_navigation_row = left && self.rail_overhang(window) > 0.;
        // Its rule leaves the corner to the rail divider's curve.
        let title_curve = if self.rail_meets_title_rule() {
            crate::liquid::PANE_RADIUS
        } else {
            0.
        };
        let sidebar = (sidebar_width > 0.).then(|| {
            div()
                .id("sidebar-frame")
                .debug_selector(|| "sidebar-frame".into())
                .relative()
                .w(px(sidebar_width))
                .h_full()
                .flex_shrink_0()
                .overflow_hidden()
                // Rounded toward its left neighbour like the pane, so its line
                // curves at the top and bottom; on the left, the rail draws it.
                .rounded_l(px(12.))
                .when(!left, |d| d.border_l_1())
                .border_color(crate::right_panel::stroke())
                .child(
                    div()
                        .absolute()
                        .map(|d| if left { d.left_0() } else { d.right_0() })
                        .top_0()
                        .w(px(self.workspace.right_panel_width))
                        .h_full()
                        .flex()
                        .flex_col()
                        .opacity(self.sidebar_fraction)
                        // A collapsed rail leaves navigation over the left sidebar.
                        // Give it a row above the project switcher and its actions.
                        .when(sidebar_navigation_row, |d| {
                            d.child(
                                div()
                                    .debug_selector(|| "sidebar-navigation-row".into())
                                    .h(px(TITLE_BAR_HEIGHT))
                                    .flex_shrink_0()
                                    .pl(px(title_curve))
                                    .child(
                                        div()
                                            .size_full()
                                            .border_b_1()
                                            .border_color(crate::right_panel::stroke()),
                                    ),
                            )
                        })
                        .child(div().h(px(TITLE_BAR_HEIGHT)).flex_shrink_0().child(header))
                        .child(div().flex_1().min_h_0().child(self.sidebar_contents())),
                )
        });
        let (before, after) = if left {
            (sidebar, None)
        } else {
            (None, sidebar)
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .relative()
            .when(rail_width > 0., |d| {
                d.child(
                    self.project_rail_frame.clone().cached(
                        StyleRefinement::default()
                            .w(px(rail_width))
                            .h_full()
                            .flex_shrink_0(),
                    ),
                )
            })
            .children(before)
            .child(
                div()
                    .id("main-pane-frame")
                    .debug_selector(|| "main-pane-frame".into())
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .rounded_l(px(12.))
                    // Beside a left sidebar, the pane draws its own curved edge.
                    .when(left && sidebar_width > 0., |d| {
                        d.border_l_1().border_color(crate::right_panel::stroke())
                    })
                    .bg(pane_fill())
                    .children(tab_bar)
                    .child(card),
            )
            .children(after)
            .children(self.sidebar_resize(cx))
            .children(panel.map(|panel| div().h_full().bg(pane_fill()).child(panel)))
            .child(div().absolute().top_0().left_0().child(navigation))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right(px(if left { 0. } else { sidebar_width }))
                    .h(px(TITLE_BAR_HEIGHT))
                    .children(corner),
            )
            .into_any_element()
    }
    /// Sidebar pages omit the tabs. An expanded rail leaves them at the top;
    /// collapsed navigation keeps a drag row above them for its overhang.
    /// Settings keeps its own header over any page.
    pub(super) fn bare_page(&self, cx: &gpui::App) -> bool {
        self.settings.is_none()
            && (self.active_app_view.is_some() || self.app_search.is_some() || self.bot_active(cx))
    }
    /// Bot details retain the window corner; project tools have their own header.
    pub(super) fn corner_room(&self, cx: &gpui::App) -> f32 {
        if self.bot_details_active(cx) {
            if self.workspace.right_panel_open && !self.app_view_open() {
                return self
                    .right_drawn
                    .max(if self.workspace.right_panel_width >= 312. {
                        312.
                    } else {
                        252.
                    });
            }
            return 6. + CORNER_BUTTONS_WIDTH + 12.;
        }
        // The open sidebar's header holds them instead (`classic_corner`).
        if self.sidebar_fraction > 0. {
            return 6.;
        }
        6. + self.diff_stat_room(cx) + 24. + 12.
    }
    pub(super) fn classic_corner(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The open sidebar's header has its own search and the diff stat.
        if self.sidebar_fraction > 0. {
            return div().into_any_element();
        }
        div()
            .h(px(TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .pl_1p5()
            .pr_3()
            .children(self.project_diff_stat(cx))
            .child(self.file_search_button(cx))
            .into_any_element()
    }
    fn sidebar_contents(&self) -> AnyElement {
        // Cached paint doesn't include inherited opacity in its key. Repaint
        // during the fade, even if the clip rounds to the same pixel on
        // successive frames.
        if self.sidebar_uncached() {
            self.sidebar_frame.clone().into_any_element()
        } else {
            self.sidebar_frame
                .clone()
                .cached(
                    StyleRefinement::default()
                        .w(px(self.workspace.right_panel_width))
                        .h_full(),
                )
                .into_any_element()
        }
    }
    fn sidebar_resize(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        (self.workspace.sidebar_visible
            && self.sidebar_fraction > 0.
            && self.sidebar_slide.is_none())
        .then(|| {
            // Drag the sidebar's separator on the pane's side.
            let border = self.workspace.right_panel_width - 0.5;
            div()
                .id("sidebar-resize")
                .debug_selector(|| "sidebar-resize".into())
                .absolute()
                .map(|d| {
                    if !self.workspace.preferences.sidebar_right {
                        d.left(px(self.classic_rail_width() + border - 2.))
                    } else {
                        d.right(px(border - 2.))
                    }
                })
                .w(px(4.))
                .h_full()
                .cursor(CursorStyle::ResizeLeftRight)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.resize = Some((event.position.x, this.workspace.right_panel_width));
                        cx.stop_propagation();
                    }),
                )
                .into_any_element()
        })
    }
    /// The workspace body, its error banner and the right panel beside them.
    fn workspace_content(
        &mut self,
        right_shown: bool,
        right_width: f32,
        full: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if let Some(search) = &self.app_search {
            return div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .flex()
                .overflow_hidden()
                .child(search.view.clone());
        }
        if let Some(settings) = &self.settings {
            return div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .flex()
                .overflow_hidden()
                .child(settings.clone());
        }
        let panel: Option<Entity<RightPanel>> = self
            .right_panel
            .clone()
            .filter(|_| right_shown && self.settings.is_none());
        div()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .child(self.background_card.probe())
            .child(
                div().flex_1().min_w_0().h_full().flex().flex_col().child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        // Every project lands on a blank session, as in
                        // vorkspace; this is empty only while one is created.
                        .child({
                            if let Some(bots) = self.bots.clone().filter(|m| {
                                m.read(cx).active.is_some() || m.read(cx).classic_channels_open()
                            }) {
                                let session = bots.read(cx).active_notification_session(cx);
                                let linked = session.and_then(|id| self.linked_notice(id, cx));
                                div()
                                    .relative()
                                    .size_full()
                                    .child(bots)
                                    .children(linked.map(|notice| {
                                        div()
                                            .absolute()
                                            .top(px(12.))
                                            .right(px(12.))
                                            .w(px(320.))
                                            .max_w_full()
                                            .child(notice)
                                    }))
                                    .into_any_element()
                            } else {
                                let body = self.workspace_body(window, cx);
                                self.with_dock(body, window, cx)
                            }
                        }),
                ),
            )
            .when_some(panel, |d, panel| {
                d.child(self.right_panel_frame(panel, right_width, full, cx))
            })
    }
    /// Modal.tsx at size `sm` with `fitViewport`: 420px, centred.
    pub(super) fn resource_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.resources.read(cx).is_open() {
            return None;
        }
        let mut sessions: HashMap<i64, SessionInfo> = self
            .sessions
            .iter()
            .map(|(id, session)| {
                let summary = &session.read(cx).summary;
                let info = SessionInfo {
                    title: summary.title.clone().into(),
                    provider: summary.provider.clone(),
                };
                (*id, info)
            })
            .collect();
        if let Some(m) = &self.bots {
            sessions.extend(m.read(cx).resource_sessions(cx));
        }
        let content = self
            .resources
            .update(cx, |monitor, cx| monitor.dialog(&sessions, window, cx));
        let viewport = window.viewport_size();
        let width = 420f32.min(viewport.width.as_f32() - 24.);
        let height = (self.resources.read(cx).measured_height().unwrap_or(240.) + 2.)
            .min(viewport.height.as_f32() - 32.);
        let anchor = Anchor::Point(gpui::point(
            px((viewport.width.as_f32() - width) / 2.),
            px((viewport.height.as_f32() - height) / 2.),
        ));
        let monitor = self.resources.downgrade();
        Some(
            div()
                .absolute()
                .size_full()
                .bg(black(0.4))
                .child(
                    popover("running-work-modal", anchor)
                        .modal_surface(self.workspace.classic_layout)
                        .width(width)
                        .gap(0.)
                        .max_height(viewport.height.as_f32() - 32.)
                        .priority(60)
                        .on_dismiss(move |window, cx| {
                            let _ = monitor.update(cx, |monitor, cx| monitor.close(window, cx));
                        })
                        .child(content),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Instant;
    use crate::{bots::Bots, shell::tests::window};
    use gpui::{Focusable, TestAppContext, WindowHandle};
    use std::time::Duration;

    fn pane_has_left_border(
        window: WindowHandle<Shell>,
        visual: &mut gpui::VisualTestContext,
    ) -> bool {
        let pane = visual.debug_bounds("main-pane-frame").unwrap();
        window
            .update(visual, |_, window, _| {
                let scale = window.scale_factor();
                window.painted_quads().iter().any(|quad| {
                    (quad.bounds.left().0 - pane.left().as_f32() * scale).abs() < 0.5
                        && (quad.bounds.top().0 - pane.top().as_f32() * scale).abs() < 0.5
                        && (quad.bounds.size.width.0 - pane.size.width.as_f32() * scale).abs() < 0.5
                        && (quad.bounds.size.height.0 - pane.size.height.as_f32() * scale).abs()
                            < 0.5
                        && quad.border_widths.left.0 > 0.
                        && quad.border_color.a > 0.
                })
            })
            .unwrap()
    }

    fn mixed_window(cx: &mut TestAppContext) -> (WindowHandle<Shell>, Entity<Bots>, String, i64) {
        let (window, _, session) = window(cx);
        let (manager, id) = window
            .update(cx, |shell, w, cx| {
                // Channels are experimental and hidden by default.
                shell.workspace.preferences.channels = true;
                shell.apply_preferences(cx);
                let manager = shell.bot_manager();
                manager.update(cx, |m, cx| {
                    m.create_named(Vec::new(), w, cx);
                    m.dismiss_settings(w, cx);
                });
                let id = manager.read(cx).active.clone().unwrap();
                (manager, id)
            })
            .unwrap();
        cx.run_until_parked();
        (window, manager, id, session)
    }

    #[gpui::test]
    fn the_sidebar_sits_beside_the_rail_by_default_or_switches_right(cx: &mut TestAppContext) {
        crate::clock::use_test_clock(cx);
        let (window, _, session) = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1400.), px(900.)));
        window
            .update(visual, |shell, w, cx| {
                shell.open(session, w, cx);
                shell.set_sidebar_visible(true, cx);
                // The sidebar sits beside the rail by default.
                shell.workspace.preferences.sidebar_right = false;
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        let rail = visual.debug_bounds("classic-project-rail").unwrap();
        let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
        let pane = visual.debug_bounds("main-pane-frame").unwrap();
        let strip = visual.debug_bounds("title-tab-strip").unwrap();
        assert_eq!(sidebar.left(), rail.right());
        assert_eq!(pane.left(), sidebar.right());
        assert!(pane_has_left_border(window, visual));
        assert!(
            strip.left() >= pane.left(),
            "tabs head the pane, after the sidebar"
        );
        // Its separator faces the pane.
        let sash = visual.debug_bounds("sidebar-resize").unwrap();
        assert!((sash.center().x - sidebar.right()).abs() <= px(2.5));
        // Switched to the right, it follows the pane.
        window
            .update(visual, |shell, _, cx| {
                shell.workspace.preferences.sidebar_right = true;
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let pane = visual.debug_bounds("main-pane-frame").unwrap();
        let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
        assert_eq!(pane.left(), rail.right());
        assert_eq!(sidebar.left(), pane.right());
        assert!(!pane_has_left_border(window, visual));
        // The rail owns the divider again once the left sidebar closes.
        window
            .update(visual, |shell, _, cx| {
                shell.workspace.preferences.sidebar_right = false;
                shell.set_sidebar_visible(false, cx);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        crate::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("sidebar-frame").is_none());
        assert!(!pane_has_left_border(window, visual));
    }

    #[gpui::test]
    fn sidebar_project_switcher_clears_navigation_across_rail_modes(cx: &mut TestAppContext) {
        crate::clock::use_test_clock(cx);
        let (window, _, session) = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1400.), px(900.)));
        window
            .update(visual, |shell, w, cx| {
                w.activate_window();
                shell.open(session, w, cx);
                shell.set_sidebar_visible(true, cx);
            })
            .unwrap();
        crate::clock::settle_animations(window, visual);
        for right in [false, true] {
            for mode in ["expanded", "compact", "hidden", "expanded"] {
                window
                    .update(visual, |shell, _, cx| {
                        shell.workspace.preferences.sidebar_right = right;
                        shell.workspace.project_rail_open = mode == "expanded";
                        if mode != "expanded" {
                            shell.workspace.preferences.collapsed_project_rail = mode.into();
                        }
                        shell.invalidate_chrome(cx);
                        cx.notify();
                    })
                    .unwrap();
                visual.run_until_parked();
                let navigation = visual.debug_bounds("titlebar-navigation").unwrap();
                let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
                let header = visual.debug_bounds("classic-workspace-header").unwrap();
                if navigation.right() > sidebar.left() {
                    assert!(
                        header.top() >= navigation.bottom(),
                        "{mode}, right={right}: {header:?} overlaps {navigation:?}"
                    );
                } else {
                    assert_eq!(header.top(), sidebar.top(), "{mode}, right={right}");
                }
                // Click the leading edge that used to sit beneath navigation.
                let project = visual.debug_bounds("sidebar-project-button").unwrap();
                visual.simulate_click(
                    gpui::point(project.left() + px(12.), project.center().y),
                    Default::default(),
                );
                visual.run_until_parked();
                window
                    .update(visual, |shell, _, cx| {
                        assert!(
                            shell.menu_open(&super::super::MenuTarget::Header),
                            "{mode}, right={right}: the project switcher remains clickable"
                        );
                        shell.close_menu(cx);
                    })
                    .unwrap();
                visual.run_until_parked();
            }
        }
    }

    #[gpui::test]
    fn the_active_tab_is_the_selected_pill_of_the_tab_strip(cx: &mut TestAppContext) {
        crate::clock::use_test_clock(cx);
        let (window, _, session) = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |shell, w, cx| shell.open(session, w, cx))
            .unwrap();
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                let id = shell.workspace.active_tab().unwrap();
                assert_eq!(
                    shell.tab_liquid.selected().as_deref(),
                    Some(format!("tab:{id}").as_str())
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn expanded_pages_have_no_title_bar_but_settings_keeps_its_header_over_them(
        cx: &mut TestAppContext,
    ) {
        crate::clock::use_test_clock(cx);
        let (window, _, _) = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |shell, w, cx| {
                shell.open_connector(crate::workspace::Connector::Github, w, cx)
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("title-tab-strip").is_none());
        assert!(visual.debug_bounds("settings-header").is_none());
        window
            .update(visual, |shell, w, cx| {
                shell.open_settings(crate::settings::Section::General, w, cx)
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("settings-header").is_some());
    }

    #[gpui::test]
    fn search_header_clears_navigation_and_keeps_its_query_across_rail_modes(
        cx: &mut TestAppContext,
    ) {
        crate::clock::use_test_clock(cx);
        let (window, _, _) = window(cx);
        let input = window
            .update(cx, |shell, w, cx| {
                w.activate_window();
                shell.workspace.preferences.sidebar_right = false;
                shell.open_app_search(w, cx);
                let input = shell
                    .app_search
                    .as_ref()
                    .unwrap()
                    .view
                    .read(cx)
                    .input
                    .clone();
                input.update(cx, |input, cx| input.set_text("query draft".into(), cx));
                input
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1400.), px(900.)));
        for mode in ["expanded", "compact", "hidden", "expanded"] {
            window
                .update(visual, |shell, w, cx| {
                    shell.workspace.project_rail_open = mode == "expanded";
                    if mode != "expanded" {
                        shell.workspace.preferences.collapsed_project_rail = mode.into();
                    }
                    w.focus(&shell.focus, cx);
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            let navigation = visual.debug_bounds("titlebar-navigation").unwrap();
            let pane = visual.debug_bounds("main-pane-frame").unwrap();
            let header = visual.debug_bounds("app-search-header").unwrap();
            assert!(!pane_has_left_border(window, visual), "{mode}");
            assert_eq!(
                header.left(),
                pane.left(),
                "{mode}: the rail owns the divider when no left sidebar is drawn"
            );
            if navigation.right() > pane.left() {
                assert!(header.top() >= navigation.bottom(), "{mode}: {header:?}");
            } else {
                assert_eq!(header.top(), pane.top(), "{mode}");
            }
            assert!(visual.debug_bounds("title-tab-strip").is_none(), "{mode}");
            // This query point lies under the overhanging buttons without clearance.
            visual.simulate_click(
                gpui::point(header.left() + px(48.), header.center().y),
                Default::default(),
            );
            visual.run_until_parked();
            window
                .update(visual, |_, w, cx| {
                    assert!(input.read(cx).focus_handle(cx).is_focused(w), "{mode}");
                    assert_eq!(input.read(cx).content, "query draft", "{mode}");
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn channels_after_bot_details_keep_a_plain_header_and_full_tab_width(cx: &mut TestAppContext) {
        crate::clock::use_test_clock(cx);
        let (window, manager, _, _) = mixed_window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
        visual.run_until_parked();
        let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("bot-overview-panel").is_some());
        assert!(visual.debug_bounds("bot-overview-tabs").is_some());

        let channels = visual.debug_bounds("classic-add-channel").unwrap();
        visual.simulate_click(channels.center(), Default::default());
        visual.run_until_parked();
        assert!(manager.read_with(visual, |manager, _| manager.classic_channels_open()));
        assert!(visual.debug_bounds("right-panel-frame").is_none());
        assert!(visual.debug_bounds("bot-overview-tabs").is_none());
        assert!(visual.debug_bounds("toggle-right-panel").is_none());
        window
            .update(visual, |shell, window, cx| {
                assert!(
                    shell.workspace.right_panel_open,
                    "the Bot's remembered preference is retained"
                );
                assert_eq!(shell.right_drawn, 0.);
                assert_eq!(shell.corner_room(cx), 42.);
                assert_eq!(
                    shell.tabs_width(window, cx),
                    1600. - shell.classic_rail_width() - 42.
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn hidden_workspace_panes_do_not_leave_a_resize_hitbox_over_global_surfaces(
        cx: &mut TestAppContext,
    ) {
        crate::clock::use_test_clock(cx);
        let (window, manager, bot, session) = mixed_window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
        window
            .update(visual, |shell, window, cx| {
                shell.open(session, window, cx);
                shell.set_sidebar_visible(true, cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let sash = visual.debug_bounds("sidebar-resize").unwrap().center();
        let width = window
            .update(visual, |shell, _, _| shell.workspace.right_panel_width)
            .unwrap();
        let moved = sash + gpui::point(px(20.), px(0.));
        visual.simulate_mouse_down(sash, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(moved, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(moved, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.right_panel_width, width - 20.)
            })
            .unwrap();

        for surface in ["GitHub", "Notes", "Search", "Bot", "Settings", "Channels"] {
            window
                .update(visual, |shell, window, cx| {
                    if shell.settings.is_some() {
                        shell.close_settings(window, cx);
                    }
                    shell.open(session, window, cx);
                    shell.set_sidebar_visible(true, cx);
                    match surface {
                        "GitHub" => shell.open_inbox(window, cx),
                        "Notes" => shell.open_notes(window, cx),
                        "Search" => shell.open_app_search(window, cx),
                        "Bot" => manager.update(cx, |manager, cx| manager.open(&bot, window, cx)),
                        "Settings" => {
                            shell.open_settings(crate::settings::Section::General, window, cx)
                        }
                        "Channels" => (),
                        _ => unreachable!(),
                    }
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            if surface == "Channels" {
                let add = visual.debug_bounds("classic-add-channel").unwrap();
                visual.simulate_click(add.center(), Default::default());
                visual.run_until_parked();
            }
            assert!(visual.debug_bounds("sidebar-frame").is_none(), "{surface}");
            assert!(visual.debug_bounds("sidebar-resize").is_none(), "{surface}");
            let (old_sash, before) = window
                .update(visual, |shell, _, _| {
                    (
                        gpui::point(
                            px(shell.viewport_width - shell.workspace.right_panel_width + 0.5),
                            px(500.),
                        ),
                        shell.workspace.right_panel_width,
                    )
                })
                .unwrap();
            visual.simulate_mouse_down(old_sash, MouseButton::Left, Default::default());
            window
                .update(visual, |shell, _, _| {
                    assert!(shell.resize.is_none(), "{surface}")
                })
                .unwrap();
            let moved = old_sash + gpui::point(px(30.), px(0.));
            visual.simulate_mouse_move(moved, MouseButton::Left, Default::default());
            visual.simulate_mouse_up(moved, MouseButton::Left, Default::default());
            visual.run_until_parked();
            window
                .update(visual, |shell, _, _| {
                    assert_eq!(shell.workspace.right_panel_width, before, "{surface}")
                })
                .unwrap();
        }
    }
    #[gpui::test]
    fn original_workspace_tabs_fit_sidebar_widths_and_keep_their_actions(cx: &mut TestAppContext) {
        use crate::workspace::SidebarTab;
        crate::clock::use_test_clock(cx);
        let (window, _, _) = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1600.), px(900.)));
        for width in [200., 260., 420., 560., 260.] {
            window
                .update(visual, |shell, _, cx| {
                    shell.workspace.right_panel_width = width;
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
            let selectors = [
                "classic-tab-sessions",
                "classic-tab-explorer",
                "classic-tab-changes",
            ];
            let tabs = selectors.map(|selector| visual.debug_bounds(selector).unwrap());
            assert_eq!(sidebar.size.width, px(width));
            assert!(tabs[0].left() >= sidebar.left());
            assert!(tabs[0].right() <= tabs[1].left());
            assert!(tabs[1].right() <= tabs[2].left());
            assert!(tabs[2].right() <= sidebar.right());
            assert!(tabs.iter().all(|tab| tab.size.width > px(0.)));
            assert!(visual.debug_bounds("right-panel-frame").is_none());
            for (index, expected) in [SidebarTab::Sessions, SidebarTab::Files, SidebarTab::Changes]
                .into_iter()
                .enumerate()
            {
                visual.simulate_click(tabs[index].center(), Default::default());
                visual.run_until_parked();
                window
                    .update(visual, |shell, _, _| {
                        assert_eq!(shell.workspace.sidebar_tab(), expected)
                    })
                    .unwrap();
            }
            let history = visual.debug_bounds("classic-source-history").unwrap();
            visual.simulate_click(history.center(), Default::default());
            visual.run_until_parked();
            window
                .update(visual, |shell, _, _| {
                    assert_eq!(shell.workspace.sidebar_tab(), SidebarTab::Graph)
                })
                .unwrap();
        }
    }
    #[gpui::test]
    fn bot_and_session_sidebars_share_width_while_retaining_their_own_visibility(
        cx: &mut TestAppContext,
    ) {
        use crate::workspace::SidebarTab;
        crate::clock::use_test_clock(cx);
        let (window, manager, bot, session) = mixed_window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("right-panel-frame").is_none());
        let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("bot-overview-panel").is_some());
        let frame = visual.debug_bounds("right-panel-frame").unwrap();
        let bounds = visual.debug_bounds("right-panel-resize").unwrap();
        assert_eq!(bounds.size.width, px(6.));
        assert!((bounds.center().x - frame.left() - px(0.5)).abs() <= px(0.5));
        for x in [bounds.left() + px(0.5), bounds.right() - px(0.5)] {
            let point = gpui::point(x, bounds.center().y);
            visual.simulate_mouse_down(point, MouseButton::Left, Default::default());
            window
                .update(visual, |shell, _, _| assert!(shell.right_resize.is_some()))
                .unwrap();
            visual.simulate_mouse_up(point, MouseButton::Left, Default::default());
            visual.run_until_parked();
            crate::clock::settle_animations(window, visual);
        }
        let handle = visual.debug_bounds("right-panel-resize").unwrap().center();
        let end = handle - gpui::point(px(30.), px(0.));
        visual.simulate_mouse_down(handle, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        let expanded = window
            .update(visual, |shell, _, _| shell.workspace.right_panel_width)
            .unwrap();
        let handle = visual.debug_bounds("right-panel-resize").unwrap().center();
        let end = handle + gpui::point(px(30.), px(0.));
        visual.simulate_mouse_down(handle, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.right_panel_width, expanded - 30.)
            })
            .unwrap();
        visual.simulate_mouse_up(
            end + gpui::point(px(20.), px(0.)),
            MouseButton::Left,
            Default::default(),
        );
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        let width = window
            .update(visual, |shell, w, cx| {
                assert!(shell.right_resize.is_none());
                crate::clock::use_test_clock(cx);
                let width = shell.workspace.right_panel_width;
                shell.open(session, w, cx);
                shell.select_classic_sidebar(SidebarTab::Files, w, cx);
                width
            })
            .unwrap();
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("right-panel-frame").is_none());
        assert!(visual.debug_bounds("bot-overview-panel").is_none());
        let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
        let explorer = visual.debug_bounds("explorer-header").unwrap();
        assert_eq!(sidebar.size.width, px(width));
        assert_eq!(sidebar.right(), px(1600.));
        assert!(explorer.left() >= sidebar.left() && explorer.right() <= sidebar.right());
        let handle = visual.debug_bounds("sidebar-resize").unwrap().center();
        let end = handle - gpui::point(px(40.), px(0.));
        visual.simulate_mouse_down(handle, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        let width = width + 40.;
        let session_sidebar = visual.debug_bounds("sidebar-frame").unwrap();
        assert_eq!(session_sidebar.size.width, px(width));
        window
            .update(visual, |_, w, cx| {
                manager.update(cx, |manager, cx| manager.open(&bot, w, cx))
            })
            .unwrap();
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        for _ in 0..3 {
            assert_eq!(
                visual.debug_bounds("right-panel-frame").unwrap(),
                session_sidebar
            );
            visual
                .update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))
                .unwrap();
        }
        crate::clock::use_test_clock(cx);
        // One sidebar fits at this size. Counting the hidden session sidebar
        // alongside Bot details must not change its visibility preference.
        visual.simulate_resize(gpui::size(px(1050.), px(1000.)));
        window
            .update(visual, |shell, w, cx| {
                assert!(shell.workspace.sidebar_visible);
                shell.open(session, w, cx);
            })
            .unwrap();
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        let session_sidebar = visual.debug_bounds("sidebar-frame").unwrap();
        assert_eq!(session_sidebar.size.width, px(width));
        window
            .update(visual, |_, w, cx| {
                manager.update(cx, |manager, cx| manager.open(&bot, w, cx));
            })
            .unwrap();
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        assert_eq!(
            visual.debug_bounds("right-panel-frame").unwrap(),
            session_sidebar
        );
        let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("right-panel-frame").is_none());
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.sidebar_tab(), SidebarTab::Files);
                assert_eq!(shell.workspace.right_panel_width, width);
                let saved = vorkspace_runtime::runtime::locked(&shell.store)
                    .unwrap()
                    .ui_state()
                    .unwrap()
                    .unwrap();
                let saved: crate::workspace::Workspace = serde_json::from_str(&saved).unwrap();
                assert_eq!(saved.right_panel_width, width);
                assert!(!saved.right_panel_open);
            })
            .unwrap();
    }
    #[gpui::test]
    fn session_sidebar_peeks_from_the_right_and_never_over_a_bot(cx: &mut TestAppContext) {
        crate::clock::use_test_clock(cx);
        let (window, manager, bot, session) = mixed_window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
        window
            .update(visual, |shell, window, cx| {
                shell.open(session, window, cx);
                shell.set_sidebar_visible(false, cx);
            })
            .unwrap();
        visual.run_until_parked();
        crate::clock::settle_animations(window, visual);
        window
            .update(visual, |shell, _, cx| shell.set_peek(true, cx))
            .unwrap();
        crate::clock::settle_animations(window, visual);
        let peek = visual.debug_bounds("sidebar-peek").unwrap();
        assert_eq!(peek.right(), px(1594.));
        assert_eq!(peek.size.width, px(crate::right_panel::WIDTH_DEFAULT));
        window
            .update(visual, |_, window, cx| {
                manager.update(cx, |manager, cx| manager.open(&bot, window, cx));
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("sidebar-peek").is_none());
    }
    #[gpui::test]
    fn switching_bots_keeps_the_same_details_spring_and_clipped_contents(cx: &mut TestAppContext) {
        let (window, manager, bot, _) = mixed_window(cx);
        let other = window
            .update(cx, |_, window, cx| {
                manager.update(cx, |manager, cx| {
                    manager.create_named(Vec::new(), window, cx);
                    manager.dismiss_settings(window, cx);
                    manager.active.clone().unwrap()
                })
            })
            .unwrap();
        crate::clock::use_test_clock(cx);
        window
            .update(cx, |shell, w, cx| shell.toggle_right_panel(w, cx))
            .unwrap();
        for ms in [35, 70, 110] {
            window
                .update(cx, |shell, w, cx| {
                    let slide = shell.right_slide.as_ref().unwrap();
                    slide.freeze_elapsed(Duration::from_millis(ms));
                    let now = Instant::now();
                    let before = slide.at(now);
                    let next = if manager.read(cx).active.as_deref() == Some(bot.as_str()) {
                        &other
                    } else {
                        &bot
                    };
                    manager.update(cx, |manager, cx| manager.open(next, w, cx));
                    assert_eq!(
                        shell.right_slide.as_ref().unwrap().at(now),
                        before,
                        "tab switches preserve the spring's position and velocity"
                    );
                    assert!(shell.workspace.right_panel_open);
                    cx.notify();
                })
                .unwrap();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx);
            })
            .unwrap();
            let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
            let frame = visual.debug_bounds("right-panel-frame").unwrap();
            let content = visual.debug_bounds("right-panel-content").unwrap();
            let width = window
                .update(visual, |shell, _, _| shell.workspace.right_panel_width)
                .unwrap();
            assert!(frame.size.width > px(0.) && frame.size.width < px(width));
            assert_eq!(content.size.width, px(width - 1.));
        }
        window
            .update(cx, |shell, w, cx| {
                shell.toggle_right_panel(w, cx);
                assert!(!shell.workspace.right_panel_open);
                assert_eq!(shell.right_slide.as_ref().unwrap().to, 0.);
                shell
                    .right_slide
                    .as_ref()
                    .unwrap()
                    .elapsed(Duration::from_millis(45));
                manager.update(cx, |manager, cx| manager.open(&other, w, cx));
                shell.toggle_right_panel(w, cx);
                assert!(shell.workspace.right_panel_open);
                assert_eq!(
                    shell.right_slide.as_ref().unwrap().to,
                    shell.workspace.right_panel_width
                );
                manager.update(cx, |m, cx| m.open(&bot, w, cx));
                shell
                    .right_slide
                    .as_ref()
                    .unwrap()
                    .elapsed(Duration::from_secs(1));
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| assert!(shell.right_slide.is_none()))
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        assert_eq!(
            visual.debug_bounds("right-panel-frame").unwrap().size.width,
            px(window
                .update(visual, |shell, _, _| shell.workspace.right_panel_width)
                .unwrap())
        );
        assert!(visual.debug_bounds("bot-overview-panel").is_some());
    }
}

#[cfg(test)]
mod navigation_fit_tests {
    use super::*;
    use crate::shell::tests::window;
    use gpui::TestAppContext;

    #[gpui::test]
    fn the_open_rail_holds_navigation_at_low_zoom(cx: &mut TestAppContext) {
        crate::clock::use_test_clock(cx);
        let (window, _, _) = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1100.), px(800.)));
        for scale in [60, 80, 100, 160] {
            window
                .update(visual, |shell, w, cx| {
                    shell.workspace.project_rail_open = true;
                    shell.workspace.project_rail_width = 180.;
                    shell.workspace.preferences.ui_scale = scale;
                    cx.set_global(shell.workspace.preferences.clone());
                    w.set_ui_scale(scale as f32 / 100., cx);
                    shell.open_app_search(w, cx);
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            crate::clock::settle_animations(window, visual);
            let rail = visual.debug_bounds("classic-project-rail").unwrap();
            let forward = visual.debug_bounds("visit-forward").unwrap();
            let pane = visual.debug_bounds("main-pane-frame").unwrap();
            let header = visual.debug_bounds("app-search-header").unwrap();
            assert!(
                forward.right() <= rail.right(),
                "{scale}%: {forward:?} {rail:?}"
            );
            // No second title row pushes the page down.
            assert_eq!(header.top(), pane.top(), "{scale}%");
            // The saved width is untouched for when zoom rises again.
            window
                .update(visual, |shell, _, _| {
                    assert_eq!(shell.workspace.project_rail_width, 180., "{scale}%")
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn the_icon_rail_divider_curves_into_the_title_rule(cx: &mut TestAppContext) {
        crate::clock::use_test_clock(cx);
        let (window, _, session) = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1000.), px(700.)));
        let lead_rule_left = |visual: &mut gpui::VisualTestContext| {
            window
                .update(visual, |_, w, _| {
                    let scale = w.scale_factor();
                    w.painted_quads()
                        .iter()
                        .filter(|q| {
                            q.border_widths.bottom.0 > 0.
                                && q.border_color.a > 0.
                                && q.bounds.top().0 == 0.
                                && (q.bounds.size.height.0 / scale - TITLE_BAR_HEIGHT).abs() < 0.5
                        })
                        .map(|q| q.bounds.left().0 / scale)
                        .fold(f32::INFINITY, f32::min)
                })
                .unwrap()
        };
        for sidebar_right in [true, false] {
            window
                .update(visual, |shell, w, cx| {
                    shell.workspace.project_rail_open = false;
                    shell.workspace.preferences.sidebar_right = sidebar_right;
                    shell.open(session, w, cx);
                    shell.set_sidebar_visible(false, cx);
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            crate::clock::settle_animations(window, visual);
            let rail = visual.debug_bounds("classic-project-rail").unwrap();
            window
                .update(visual, |shell, _, _| assert!(shell.rail_meets_title_rule()))
                .unwrap();
            // The rule leaves the corner to the divider's curve.
            assert_eq!(
                lead_rule_left(visual),
                rail.right().as_f32() + crate::liquid::PANE_RADIUS,
                "sidebar_right={sidebar_right} rail={rail:?} pane={:?}",
                visual.debug_bounds("main-pane-frame")
            );
        }
        // A left sidebar takes the overhang, and the rule, in its own row.
        window
            .update(visual, |shell, _, cx| {
                shell.set_sidebar_visible(true, cx);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        crate::clock::settle_animations(window, visual);
        let rail = visual.debug_bounds("classic-project-rail").unwrap();
        let row = visual.debug_bounds("sidebar-navigation-row").unwrap();
        let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
        let pane = visual.debug_bounds("main-pane-frame").unwrap();
        assert_eq!(row.left(), rail.right());
        assert_eq!(row.right(), sidebar.right());
        assert_eq!(
            lead_rule_left(visual),
            rail.right().as_f32() + crate::liquid::PANE_RADIUS
        );
        // The pane's tabs start at its edge; navigation does not reach it.
        // Inside the pane's own 1px border beside the sidebar.
        let strip = visual.debug_bounds("title-tab-strip").unwrap();
        assert_eq!(strip.left(), pane.left() + px(1.));
    }

    #[gpui::test]
    fn the_sidebar_tabs_rise_from_their_line(cx: &mut TestAppContext) {
        crate::clock::use_test_clock(cx);
        let (window, _, session) = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1200.), px(800.)));
        window
            .update(visual, |shell, w, cx| {
                shell.open(session, w, cx);
                shell.set_sidebar_visible(true, cx);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        crate::clock::settle_animations(window, visual);
        let selected = |visual: &mut gpui::VisualTestContext| {
            window
                .update(visual, |shell, _, _| shell.sidebar_tab_liquid.selected())
                .unwrap()
        };
        assert_eq!(selected(visual).as_deref(), Some("sidebar-tab:Sessions"));
        let changes = visual.debug_bounds("classic-tab-changes").unwrap();
        visual.simulate_click(changes.center(), Default::default());
        crate::clock::settle_animations(window, visual);
        assert_eq!(selected(visual).as_deref(), Some("sidebar-tab:Changes"));
    }

    #[gpui::test]
    fn to_dos_open_from_the_sidebar_and_the_project_switcher(cx: &mut TestAppContext) {
        use vorkspace_core::todos::TodoBoard;
        crate::clock::use_test_clock(cx);
        let (window, _, session) = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1200.), px(800.)));
        window
            .update(visual, |shell, w, cx| {
                shell.workspace.project_rail_open = true;
                shell.open(session, w, cx);
                shell.set_sidebar_visible(true, cx);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        crate::clock::settle_animations(window, visual);
        // The project's switcher has a To-dos tab for the current project.
        let tab = visual.debug_bounds("classic-tab-to-dos").unwrap();
        visual.simulate_click(tab.center(), Default::default());
        crate::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("todo-board").is_some());
        window
            .update(visual, |shell, _, cx| {
                let board = shell
                    .project_todos
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .board()
                    .cloned();
                assert_eq!(
                    board,
                    Some(TodoBoard::Project(shell.workspace.project.clone()))
                );
            })
            .unwrap();
        // The left sidebar's To-dos page starts on the personal board.
        let entry = visual.debug_bounds("sidebar-navigation-To-dos").unwrap();
        visual.simulate_click(entry.center(), Default::default());
        crate::clock::settle_animations(window, visual);
        assert!(visual.debug_bounds("todos-page").is_some());
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(
                    shell.active_app_view,
                    Some(crate::workspace::AppView::Todos)
                );
                let page = shell.todos_page.as_ref().unwrap().read(cx);
                assert_eq!(page.selected(cx), Some(TodoBoard::Personal));
            })
            .unwrap();
        // Its picker lists the project.
        let picker = visual.debug_bounds("todo-board-picker-button").unwrap();
        visual.simulate_click(picker.center(), Default::default());
        crate::clock::settle_animations(window, visual);
        let project = visual.debug_bounds("todo-board-choice-1").unwrap();
        visual.simulate_click(project.center(), Default::default());
        crate::clock::settle_animations(window, visual);
        window
            .update(visual, |shell, _, cx| {
                let page = shell.todos_page.as_ref().unwrap().read(cx);
                assert!(matches!(page.selected(cx), Some(TodoBoard::Project(_))));
            })
            .unwrap();
    }
}
