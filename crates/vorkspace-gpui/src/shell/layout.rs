// Project rail, conversation workspace, then project tools on the right.
// Each column owns its header; subscriptions live above the rail settings.
use super::{CORNER_BUTTONS_WIDTH, Shell};
use crate::{
    popover::{Anchor, popover},
    resource_pressure::SessionInfo,
    right_panel::RightPanel,
};
use gpui::{
    AnyElement, Context, CursorStyle, Entity, MouseButton, MouseDownEvent, StyleRefinement, Window,
    div, prelude::*, px,
};
use std::collections::HashMap;
use vorkspace_ui::theme::*;

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
mod tests;

#[cfg(test)]
mod navigation_fit_tests;
