use vorkspace_ui::radius;
// Project rail and sidebar on the left, then the conversation workspace.
// Each column owns its header; subscriptions live above the rail settings.
use super::Shell;
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
        // A non-classic project panel retains its saved width.
        let panel = self
            .right_panel
            .clone()
            .filter(|_| right_shown && self.settings.is_none())
            .map(|panel| self.right_panel_frame(panel, right_width, full, cx));
        // Every workspace destination shares the title row. Settings keeps
        // its own header above the parked workspace.
        let project_tools = self.has_session_panes();
        // Collapsed navigation can extend past the rail. Keep its drag row
        // above page headers so the controls cannot cover their inputs.
        let tab_bar = self.titlebar(window, cx);
        let navigation = self.navigation(window, cx);
        let rail_width = self.classic_rail_width();
        // At zero width the cached rail is not rendered, so it cannot stop
        // presentation tasks itself.
        if rail_width <= 0. {
            self.hide_project_marks(cx);
        }
        let corner =
            (self.settings.is_none() && project_tools).then(|| self.panel_toggle(window, cx));
        let header = self.classic_workspace_header(cx);
        // Sessions, Explorer and Changes sit beside the rail on the left.
        let sidebar_navigation_row = self.rail_overhang(window) > 0.;
        // Its rule leaves the corner to the rail divider's curve.
        let title_curve = if self.rail_meets_title_rule() {
            radius::SURFACE
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
                .rounded_l(px(radius::SURFACE))
                .border_color(crate::panel_kit::stroke())
                .child(
                    div()
                        .absolute()
                        .left_0()
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
                                            .border_color(crate::panel_kit::stroke()),
                                    ),
                            )
                        })
                        .child(div().h(px(TITLE_BAR_HEIGHT)).flex_shrink_0().child(header))
                        .child(div().flex_1().min_h_0().child(self.sidebar_contents())),
                )
        });
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
            .children(sidebar)
            .child(
                div()
                    .id("main-pane-frame")
                    .debug_selector(|| "main-pane-frame".into())
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .rounded_l(px(radius::SURFACE))
                    // Beside a left sidebar, the pane draws its own curved edge.
                    .when(sidebar_width > 0., |d| {
                        d.border_l_1().border_color(crate::panel_kit::stroke())
                    })
                    .when(self.workspace.project_rail_open, |d| d.bg(pane_fill()))
                    .child(tab_bar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .when(!self.workspace.project_rail_open, |d| d.bg(pane_fill()))
                            .child(card),
                    ),
            )
            .children(self.sidebar_resize(cx))
            .children(panel.map(|panel| div().h_full().bg(pane_fill()).child(panel)))
            .child(div().absolute().top_0().left_0().child(navigation))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .h(px(TITLE_BAR_HEIGHT))
                    .children(corner),
            )
            .into_any_element()
    }
    /// Project tools have their own header.
    pub(super) fn corner_room(&self, cx: &gpui::App) -> f32 {
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
    pub(super) fn sidebar_uncached(&self) -> bool {
        self.sidebar_slide.is_some()
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
                .left(px(self.classic_rail_width() + border - 2.))
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
                            let body = self.workspace_body(window, cx);
                            self.with_dock(body, window, cx)
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
