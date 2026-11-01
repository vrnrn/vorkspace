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
pub fn spinner_dots(elapsed_ms: u64) -> gpui::Div {
    dots(None, Some(elapsed_ms))
}

/// The grid in `color`, or the spinner's rows; still and solid without
/// `elapsed_ms`.
fn dots(color: Option<u32>, elapsed_ms: Option<u64>) -> gpui::Div {
    let phase = elapsed_ms.map(|ms| (ms % 1_200) as f32 / 1000. / CYCLE_SECS * TAU);
    div()
        .size(px(12.))
        .flex()
        .flex_col()
        .children((0..3).map(move |row| {
            div().flex().children((0..3).map(move |col| {
                let (alpha, scale) = match phase {
                    Some(phase) => {
                        let wave =
                            (0.5 - 0.5 * (phase - (col + row) as f32 * 0.9).cos()).powf(3.52);
                        let peak = 0.93 * (1. - 0.65 * (col as f32 - row as f32).abs() / 2.);
                        (0.07 + peak * wave, 0.89 + 0.11 * peak * wave)
                    }
                    None => (0.9, 1.),
                };
                let mut color: Hsla = rgb(color.unwrap_or(ROWS[row])).into();
                color.a = alpha;
                div()
                    .size(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(px(2.5 * scale)).rounded_full().bg(color))
            }))
        }))
}

/// The grid, still and solid in `color`: a mark for a channel.
pub fn color_dots(color: u32) -> gpui::Div {
    dots(Some(color), None)
}

/// A project's mark: the working grid in its colour, still while idle and
/// waving while any of its sessions works. Only a working mark ticks.
pub struct ProjectMark {
    color: u32,
    frame: usize,
    animation: Option<Task<()>>,
}
impl ProjectMark {
    pub fn new(color: u32) -> Self {
        Self {
            color,
            frame: 0,
            animation: None,
        }
    }
    /// A removed row may still be retained by a cached element or a caller.
    /// Cancel its presentation task explicitly before releasing the rail owner.
    pub fn hide(&mut self) {
        self.animation = None;
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn animation_state(&self) -> (usize, bool) {
        (self.frame, self.animation.is_some())
    }
    pub fn set(&mut self, color: u32, working: bool, cx: &mut Context<Self>) {
        if self.color != color {
            self.color = color;
            cx.notify();
        }
        if working == self.animation.is_some() {
            return;
        }
        self.frame = 0;
        self.animation = working.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(TICK_MS))
                        .await;
                    let ticked = this.update(cx, |this, cx| {
                        this.frame = this.frame.wrapping_add(1);
                        cx.notify();
                    });
                    if ticked.is_err() {
                        break;
                    }
                }
            })
        });
        cx.notify();
    }
}
impl Render for ProjectMark {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let working = self.animation.is_some();
        dots(
            Some(self.color),
            working.then(|| self.frame as u64 * TICK_MS),
        )
        .flex_shrink_0()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Working,
    Done,
    /// Waiting on an approval or a question.
    Attention,
}

/// A small cached view so animation leaves the conversation untouched.
pub struct SessionMark {
    side: f32,
    provider: String,
    /// The session's project colour, for its working grid.
    color: Option<u32>,
    state: State,
    frame: usize,
    animation: Option<Task<()>>,
}
impl SessionMark {
    pub fn new(provider: String) -> Self {
        Self {
            side: 16.,
            provider,
            color: None,
            state: State::Idle,
            frame: 0,
            animation: None,
        }
    }
    /// A title tab's mark. Tabs paint inside their own cached frame, so they
    /// can't share the sidebar's entity: a notify only repaints the cached
    /// views along the one path the entity was last drawn in.
    pub fn tab(provider: String) -> Self {
        Self {
            side: 14.,
            ..Self::new(provider)
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn state(&self) -> State {
        self.state
    }
    pub fn set_size(&mut self, side: f32, cx: &mut Context<Self>) {
        if self.side != side {
            self.side = side;
            cx.notify();
        }
    }
    pub fn set_color(&mut self, color: u32, cx: &mut Context<Self>) {
        if self.color != Some(color) {
            self.color = Some(color);
            cx.notify();
        }
    }
    pub fn set(&mut self, provider: &str, state: State, cx: &mut Context<Self>) {
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
            State::Working => dots(self.color, Some(self.frame as u64 * TICK_MS))
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
