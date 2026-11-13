use vorkspace_ui::radius;
// Project rail and sidebar on the left, then the conversation workspace.
// Each column owns its header; subscriptions live above the rail settings.
use super::Shell;
use crate::{
    popover::{Anchor, popover},
    resource_pressure::{SessionInfo, StopSession},
};
use gpui::{
    AnyElement, Context, CursorStyle, MouseButton, MouseDownEvent, StyleRefinement, Window, div,
    prelude::*, px,
};
use std::collections::HashMap;
use vorkspace_ui::theme::*;

impl Shell {
    pub(super) fn docked_sidebar_width(&self) -> f32 {
        self.sidebar.content_width * self.sidebar.fraction
    }

    pub(super) fn layout_body(
        &mut self,
        sidebar_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card = self.workspace_content(window, cx);
        // Every workspace destination shares the title row. Settings keeps
        // its own header above the parked workspace.
        // Collapsed navigation can extend past the rail. Keep its drag row
        // above page headers so the controls cannot cover their inputs.
        let tab_bar = self.titlebar(window, cx);
        let navigation = self.navigation(window, cx);
        let rail_width = self.classic_rail_width();
        if rail_width <= 0. {
            self.sidebar
                .live_agents
                .update(cx, |activity, _| activity.hide());
        }
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
                        .w(px(self.sidebar.content_width))
                        .h_full()
                        .flex()
                        .flex_col()
                        .opacity(self.sidebar.fraction)
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
            .child(div().absolute().top_0().left_0().child(navigation))
            .into_any_element()
    }
    pub(super) fn sidebar_uncached(&self) -> bool {
        self.sidebar.slide.is_some()
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
                        .w(px(self.sidebar.content_width))
                        .h_full(),
                )
                .into_any_element()
        }
    }
    fn sidebar_resize(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        (self.workspace.sidebar_visible
            && self.sidebar.fraction > 0.
            && self.sidebar.content_width == self.workspace.right_panel_width
            && self.sidebar.slide.is_none())
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
                        this.sidebar.resize =
                            Some((event.position.x, this.workspace.right_panel_width));
                        cx.stop_propagation();
                    }),
                )
                .into_any_element()
        })
    }
    fn workspace_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        if let Some(settings) = &self.pages.settings {
            return div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .flex()
                .overflow_hidden()
                .child(settings.clone());
        }
        div().flex_1().min_h_0().overflow_hidden().flex().child(
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
    }
    /// A small modal fitted to the viewport: 420px, centred.
    pub(super) fn resource_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.resources.read(cx).is_open() {
            return None;
        }
        let roster = self
            .app_core
            .as_ref()
            .and_then(|core| core.read(cx).bot_roster.read(cx).committed());
        let sessions: HashMap<i64, SessionInfo> = self
            .resources
            .read(cx)
            .displayed_sessions()
            .filter_map(|id| {
                let local_bot = self
                    .bots
                    .as_ref()
                    .and_then(|bots| bots.read(cx).calling_view(id, cx));
                let summary = self
                    .sessions
                    .get(&id)
                    .or(local_bot.as_ref())
                    .map(|view| &view.read(cx).summary)
                    .or_else(|| self.session_summary(id, cx));
                let info = if let Some(summary) = summary {
                    SessionInfo {
                        title: summary.title.clone().into(),
                        provider: Some(summary.provider.clone()),
                    }
                } else {
                    SessionInfo {
                        title: roster?.of_session(id)?.name.clone().into(),
                        provider: None,
                    }
                };
                Some((id, info))
            })
            .collect();
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
                .bg(vorkspace_ui::fluid::scrim(0.4))
                .child(
                    popover("running-work-modal", anchor)
                        .modal_surface()
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

    pub(crate) fn show_running_work(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.management_modal_focus(cx).is_none() {
            self.resources
                .update(cx, |resources, cx| resources.open(window, cx));
        }
    }

    pub(super) fn stop_resource_turn(&self, request: &StopSession, cx: &mut Context<Self>) {
        let core = self.app_core.clone();
        let monitor = self.resources.downgrade();
        let request = request.clone();
        // Let this Shell update finish before the conversation publishes Stop.
        cx.defer(move |cx| {
            let store = core.as_ref().map(|core| core.read(cx).store.clone());
            let stopped = store.is_some_and(|store| {
                crate::conversation::turn_driver::stop_running(
                    &store,
                    request.session,
                    &request.turn,
                    cx,
                )
            });
            let _ = monitor.update(cx, |monitor, cx| {
                monitor.finish_session_stop(&request, stopped, cx)
            });
        });
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod navigation_fit_tests;
