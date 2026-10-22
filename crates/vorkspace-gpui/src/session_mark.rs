//! A session's provider mark, working spinner, circled done check and amber
//! attention mark, for sidebar rows and title tabs.
use crate::theme::*;
use gpui::{AnyElement, Context, Hsla, Render, Role, Task, Window, div, prelude::*, px, rgb};
use std::f32::consts::TAU;
use std::time::Duration;

/// The spinner repaints at about 30fps so its fades stay smooth.
const TICK_MS: u64 = 33;
/// One wave across the grid, as TerminalSpinner's `vorkspace-spinner-wave`.
const CYCLE_SECS: f32 = 1.2;
/// Slate, peach and pink, top row to bottom.
const ROWS: [u32; 3] = [0x6f84a3, 0xe8b583, 0xe47fa6];

/// TerminalSpinner's 12px 3×3 grid at `elapsed_ms` into its animation: a soft
/// wave washes from the top-left corner to the bottom-right, and dots off the
/// center diagonal peak dimmer.
pub(crate) fn spinner_dots(elapsed_ms: u64) -> gpui::Div {
    let phase = (elapsed_ms % 1_200) as f32 / 1000. / CYCLE_SECS * TAU;
    div()
        .size(px(12.))
        .flex()
        .flex_col()
        .children((0..3).map(|row| {
            div().flex().children((0..3).map(move |col| {
                let wave = (0.5 - 0.5 * (phase - (col + row) as f32 * 0.9).cos()).powf(3.52);
                let peak = 0.93 * (1. - 0.65 * (col as f32 - row as f32).abs() / 2.);
                let alpha = 0.07 + peak * wave;
                let mut color: Hsla = rgb(ROWS[row]).into();
                color.a = alpha;
                div()
                    .size(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(2.5 * (0.89 + 0.11 * peak * wave)))
                            .rounded_full()
                            .bg(color),
                    )
            }))
        }))
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Idle,
    Working,
    Done,
    /// Waiting on an approval or a question.
    Attention,
}

/// A small cached view so animation leaves the conversation untouched.
pub(crate) struct SessionMark {
    side: f32,
    provider: String,
    state: State,
    frame: usize,
    animation: Option<Task<()>>,
}
impl SessionMark {
    pub(crate) fn new(provider: String) -> Self {
        Self {
            side: 16.,
            provider,
            state: State::Idle,
            frame: 0,
            animation: None,
        }
    }
    /// A title tab's mark. Tabs paint inside their own cached frame, so they
    /// can't share the sidebar's entity: a notify only repaints the cached
    /// views along the one path the entity was last drawn in.
    pub(crate) fn tab(provider: String) -> Self {
        Self {
            side: 14.,
            ..Self::new(provider)
        }
    }
    #[cfg(test)]
    pub(crate) fn state(&self) -> State {
        self.state
    }
    pub(crate) fn set_size(&mut self, side: f32, cx: &mut Context<Self>) {
        if self.side != side {
            self.side = side;
            cx.notify();
        }
    }
    pub(crate) fn set(&mut self, provider: &str, state: State, cx: &mut Context<Self>) {
        if self.state == state && self.provider == provider {
            return;
        }
        self.provider = provider.to_owned();
        if self.state != state {
            self.animation = None;
            self.frame = 0;
            if state == State::Working {
                self.animation = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(TICK_MS))
                            .await;
                        if this
                            .update(cx, |this, cx| {
                                this.frame = this.frame.wrapping_add(1);
                                cx.notify();
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
            }
            self.state = state;
        }
        cx.notify();
    }
}
impl Render for SessionMark {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let side = self.side;
        let mark: AnyElement = match self.state {
            State::Idle => provider_icon(&self.provider, side).into_any_element(),
            State::Working => spinner_dots(self.frame as u64 * TICK_MS)
                .id("session-working")
                .role(Role::Image)
                .aria_label("Working")
                .into_any_element(),
            // Tailwind teal-400 and amber-400, as TitleBar.tsx.
            State::Done => div()
                .id("session-done")
                .role(Role::Image)
                .aria_label("Done")
                .child(icon("check-circle-2", 14.).text_color(rgb(0x2dd4bf)))
                .into_any_element(),
            State::Attention => div()
                .id("session-attention")
                .role(Role::Image)
                .aria_label("Needs approval")
                .child(icon("alert-circle", 14.).text_color(rgb(0xfbbf24)))
                .into_any_element(),
        };
        div()
            .debug_selector(|| "session-provider-mark".into())
            .size(px(side))
            .flex()
            .items_center()
            .justify_center()
            .child(mark)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    #[gpui::test]
    fn spinner_advances_each_tick_and_stops_after_completion(cx: &mut TestAppContext) {
        let mark = cx.new(|_| SessionMark::new("codex".into()));
        mark.update(cx, |mark, cx| mark.set("codex", State::Working, cx));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(TICK_MS));
        cx.run_until_parked();
        assert_eq!(mark.read_with(cx, |mark, _| mark.frame), 1);
        mark.update(cx, |mark, cx| mark.set("codex", State::Done, cx));
        cx.executor().advance_clock(Duration::from_millis(160));
        cx.run_until_parked();
        mark.read_with(cx, |mark, _| {
            assert_eq!(mark.frame, 0);
            assert!(mark.animation.is_none());
        });
    }
}
