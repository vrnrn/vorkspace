//! Habit list and editor navigation inside Bot settings.
use super::panel_stack::{PanelStack, Pose};
use super::*;
use vorkspace_ui::clock::Instant;

pub(crate) struct HabitPanel {
    manager: WeakEntity<Bots>,
    focus: gpui::FocusHandle,
    stack: PanelStack,
    _observation: Subscription,
}

impl HabitPanel {
    pub(super) fn new(manager: Entity<Bots>, cx: &mut Context<Self>) -> Self {
        Self {
            manager: manager.downgrade(),
            focus: cx.focus_handle(),
            stack: PanelStack::default(),
            _observation: cx.observe(&manager, |_, _, cx| cx.notify()),
        }
    }

    pub(super) fn reset(&mut self, cx: &mut Context<Self>) {
        self.stack = PanelStack::default();
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn elapsed(&mut self, elapsed: std::time::Duration, cx: &mut Context<Self>) {
        self.stack.elapsed(elapsed);
        cx.notify();
    }
}

fn layer(id: String, pose: Pose, interactive: bool, body: AnyElement) -> AnyElement {
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .absolute()
        .top_0()
        .left(gpui::relative(pose.x))
        .w_full()
        .h_full()
        .opacity(pose.opacity)
        .flex()
        .flex_col()
        .occlude()
        .child(body)
        .when(!interactive, |d| {
            d.child(div().absolute().size_full().occlude())
        })
        .into_any_element()
}

impl Focusable for HabitPanel {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

impl Render for HabitPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let state = self
            .manager
            .read_with(cx, |m, _| {
                m.current().cloned().map(|bot| (bot, m.route.clone()))
            })
            .ok()
            .flatten();
        let mut layers = Vec::new();
        if let Some((bot, route)) = state {
            let changed = self.stack.sync(&bot.id, route, now);
            if changed && self.focus.contains_focused(window, cx) {
                window.focus(&self.focus, cx);
            }
            let frame = self.stack.frame(now);
            if frame.animating {
                window.request_animation_frame();
            }
            let _ = self.manager.update(cx, |m, cx| {
                layers.push(layer(
                    "bot-detail-base".into(),
                    frame.base,
                    !frame.covered,
                    m.habits_settings(&bot, cx),
                ));
                for page in frame.pages {
                    let key = format!("bot-detail-page-{}", page.route.key());
                    let body = m.route_page(&bot, page.route, window, cx);
                    layers.push(layer(key, page.pose, page.interactive, body));
                }
            });
        }
        div()
            .id("bot-habits-panel")
            .debug_selector(|| "bot-habits-panel".into())
            .role(Role::Complementary)
            .aria_label("Habits")
            .track_focus(&self.focus)
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("bot-detail-stack")
                    .debug_selector(|| "bot-detail-stack".into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .children(layers),
            )
    }
}
