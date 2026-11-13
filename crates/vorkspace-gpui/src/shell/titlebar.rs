//! Titlebar layout: navigation controls, project tools and their reserved widths.
use super::*;
use vorkspace_ui::radius;

impl Shell {
    /// The title bar's navigation (`data-inset-title-nav`): as wide as the
    /// open sidebar, with history at its far edge; with the sidebar closed,
    /// just the traffic lights' gap and its three buttons.
    pub(super) fn traffic_lights(&self, window: &Window) -> f32 {
        traffic_light_inset(window.ui_scale(), self.window.fullscreen)
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
            + 2. * Self::navigation_button_size(window)
            + self.navigation_padding();
        let minimum = (minimum * scale).ceil() / scale;
        self.classic_rail_width().max(minimum)
    }
    /// Room before the tabs where navigation overhangs the pane.
    pub(super) fn strip_lead(&self, window: &Window) -> f32 {
        // A left sidebar takes the overhang in its own row (`classic_body`).
        let sidebar = self.docked_sidebar_width();
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
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&toggle_focus, cx);
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_project_rail(cx))),
            )
            .child(
                // The explorer sidebar is its own persistent toggle: it stays
                // as the user leaves it, whatever the main pane shows.
                icon_button("toggle-explorer", TOGGLE_EXPLORER_LABEL, "folder")
                    .debug_selector(|| "toggle-explorer".into())
                    .size(button_size)
                    .when(self.workspace.sidebar_visible, |b| b.bg(content(0.08)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx))),
            )
            .child(titlebar_drag_area().flex_1())
    }
    pub(super) fn titlebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(settings) = self.pages.settings.clone() {
            let lead = self.strip_lead(window);
            let header = settings.update(cx, |view, cx| view.header(cx));
            return div()
                .h(px(TITLE_BAR_HEIGHT))
                .flex_shrink_0()
                .flex()
                .items_center()
                .border_b_1()
                .border_color(content(0.1))
                // The floating navigation owns gestures over this space.
                .child(div().h_full().w(px(lead)).flex_shrink_0())
                .child(header)
                .into_any_element();
        }
        let curve = if self.rail_meets_title_rule() && self.sidebar.fraction <= 0. {
            radius::SURFACE
        } else {
            0.
        };
        div()
            .h(px(TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .child(
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
                    ),
            )
            .child(
                self.tabs_frame.clone().cached(
                    StyleRefinement::default()
                        .w(px(self.tabs_width(window)))
                        .h(px(TITLE_BAR_HEIGHT)),
                ),
            )
            .child(
                titlebar_drag_area()
                    .w(px(STRIP_TRAIL))
                    .h_full()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(crate::panel_kit::stroke()),
            )
            .into_any_element()
    }
}
