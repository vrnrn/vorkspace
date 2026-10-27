//! Animation time. Production reads the monotonic system clock. UI tests bind
//! GPUI's executor clock: reads are stable and `advance_clock` moves both timers
//! and animations together. No transition is skipped or aged at first paint.
#[cfg(test)]
pub(crate) use fake::Instant;
#[cfg(test)]
use gpui::AppContext;
#[cfg(not(test))]
pub(crate) use std::time::Instant;

/// Only perpetual decorative frame requests may be suppressed in UI tests.
/// Finite transitions must still create, sample, retarget and finish normally.
pub(crate) fn suppress_endless_animation_frames() -> bool {
    #[cfg(test)]
    {
        fake::EXECUTOR.with(|executor| executor.borrow().is_some()) && !fake::ENDLESS_FRAMES.get()
    }
    #[cfg(not(test))]
    {
        false
    }
}

#[cfg(test)]
pub(crate) trait TestClockContext {
    fn animation_executor(&self) -> gpui::BackgroundExecutor;
}
#[cfg(test)]
impl TestClockContext for gpui::App {
    fn animation_executor(&self) -> gpui::BackgroundExecutor {
        self.background_executor().clone()
    }
}
#[cfg(test)]
impl<T> TestClockContext for gpui::Context<'_, T> {
    fn animation_executor(&self) -> gpui::BackgroundExecutor {
        self.background_executor().clone()
    }
}
#[cfg(test)]
impl TestClockContext for gpui::TestAppContext {
    fn animation_executor(&self) -> gpui::BackgroundExecutor {
        self.executor()
    }
}
#[cfg(test)]
impl TestClockContext for gpui::HeadlessAppContext {
    fn animation_executor(&self) -> gpui::BackgroundExecutor {
        self.background_executor().clone()
    }
}
#[cfg(test)]
impl TestClockContext for gpui::VisualTestContext {
    fn animation_executor(&self) -> gpui::BackgroundExecutor {
        self.executor()
    }
}
#[cfg(test)]
pub(crate) fn use_test_clock(cx: &impl TestClockContext) {
    let next = cx.animation_executor();
    let changed = fake::EXECUTOR.with(|executor| {
        let mut executor = executor.borrow_mut();
        let changed = executor.as_ref().is_none_or(|previous| {
            !std::sync::Arc::ptr_eq(
                previous.scheduler_executor().scheduler(),
                next.scheduler_executor().scheduler(),
            )
        });
        *executor = Some(next);
        changed
    });
    if changed {
        crate::bots::avatar::reset_test_clock();
    }
    fake::ENDLESS_FRAMES.set(false);
}

/// Bind the executor clock and keep perpetual decorative frames, so a test can
/// measure their cadence itself. Suppression returns when the guard drops.
#[cfg(test)]
#[must_use]
pub(crate) fn use_test_clock_with_endless_frames(cx: &impl TestClockContext) -> EndlessFrames {
    use_test_clock(cx);
    fake::ENDLESS_FRAMES.set(true);
    EndlessFrames
}
#[cfg(test)]
pub(crate) struct EndlessFrames;
#[cfg(test)]
impl Drop for EndlessFrames {
    fn drop(&mut self) {
        fake::ENDLESS_FRAMES.set(false);
    }
}

/// Paint the initial state, then finish a finite pane transition and its body
/// reveal on separate frames. Geometry assertions call this after their action;
/// installing a clock alone never settles an animation.
#[cfg(test)]
pub(crate) fn settle_animations(
    window: impl Into<gpui::AnyWindowHandle>,
    cx: &mut gpui::TestAppContext,
) {
    use_test_clock(cx);
    let window = window.into();
    for elapsed in [
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(1),
    ] {
        cx.executor().advance_clock(elapsed);
        cx.update_window(window, |_, window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
}

/// The same paint/advance lifecycle for the opt-in offscreen capture tests.
#[cfg(test)]
pub(crate) fn settle_headless_animations(
    window: impl Into<gpui::AnyWindowHandle>,
    cx: &mut gpui::HeadlessAppContext,
) {
    use_test_clock(cx);
    let window = window.into();
    for elapsed in [
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(1),
    ] {
        cx.advance_clock(elapsed);
        cx.update_window(window, |_, window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
}

#[cfg(test)]
mod fake {
    use std::{
        cell::RefCell,
        ops::{Add, AddAssign, Sub, SubAssign},
        time::Duration,
    };

    thread_local! {
        pub(super) static EXECUTOR: RefCell<Option<gpui::BackgroundExecutor>> = const { RefCell::new(None) };
        pub(super) static ENDLESS_FRAMES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub(crate) struct Instant(std::time::Instant);

    impl Instant {
        pub(crate) fn now() -> Self {
            Self(EXECUTOR.with(|executor| {
                executor
                    .borrow()
                    .as_ref()
                    .map_or_else(std::time::Instant::now, gpui::BackgroundExecutor::now)
            }))
        }
        pub(crate) fn elapsed(&self) -> Duration {
            Self::now().duration_since(*self)
        }
        pub(crate) fn duration_since(&self, earlier: Self) -> Duration {
            self.0.duration_since(earlier.0)
        }
        pub(crate) fn saturating_duration_since(&self, earlier: Self) -> Duration {
            self.0.saturating_duration_since(earlier.0)
        }
    }
    impl Add<Duration> for Instant {
        type Output = Self;
        fn add(self, rhs: Duration) -> Self {
            Self(self.0 + rhs)
        }
    }
    impl Sub<Duration> for Instant {
        type Output = Self;
        fn sub(self, rhs: Duration) -> Self {
            Self(self.0 - rhs)
        }
    }
    impl Sub for Instant {
        type Output = Duration;
        fn sub(self, rhs: Self) -> Duration {
            self.0.duration_since(rhs.0)
        }
    }
    impl AddAssign<Duration> for Instant {
        fn add_assign(&mut self, rhs: Duration) {
            self.0 += rhs;
        }
    }
    impl SubAssign<Duration> for Instant {
        fn sub_assign(&mut self, rhs: Duration) {
            self.0 -= rhs;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[gpui::test]
    fn reads_are_stable_and_only_executor_advancement_changes_time(cx: &mut gpui::TestAppContext) {
        use_test_clock(cx);
        let start = Instant::now();
        for _ in 0..1000 {
            assert_eq!(Instant::now(), start);
            assert_eq!(start.elapsed(), Duration::ZERO);
        }
        cx.executor().advance_clock(Duration::from_millis(37));
        for _ in 0..1000 {
            assert_eq!(Instant::now(), start + Duration::from_millis(37));
            assert_eq!(start.elapsed(), Duration::from_millis(37));
        }
    }
}
