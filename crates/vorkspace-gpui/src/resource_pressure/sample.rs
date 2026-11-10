//! One sampler owns the machine-wide pressure and its UI-thread stall clock.
use gpui::{App, AppContext as _, Context, Entity, Task};
use std::time::{Duration, Instant};
use vorkspace_core::resources::{self, Pressure, Sample};

const TICK: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_secs(3);

struct Installed(Entity<MachinePressure>);
impl gpui::Global for Installed {}

pub(super) struct MachinePressure {
    pub(super) pressure: Pressure,
    _sampler: Option<Task<()>>,
}
impl MachinePressure {
    pub(super) fn get(cx: &mut App) -> Entity<Self> {
        if let Some(installed) = cx.try_global::<Installed>() {
            return installed.0.clone();
        }
        let sampler = cx.new(Self::new);
        cx.set_global(Installed(sampler.clone()));
        sampler
    }

    fn new(cx: &mut Context<Self>) -> Self {
        // Tests drive `observe` directly rather than the machine's load.
        let sampler = (!cfg!(test)).then(|| {
            cx.spawn(async move |this, cx| {
                let mut last_tick = Instant::now();
                let mut last_poll = last_tick;
                let mut stalls = Duration::ZERO;
                loop {
                    cx.background_executor().timer(TICK).await;
                    let now = Instant::now();
                    let late = now
                        .saturating_duration_since(last_tick)
                        .saturating_sub(TICK);
                    last_tick = now;
                    // A hidden app's timers are throttled; that is not a stall.
                    if cx.update(|cx| cx.active_window().is_none()) {
                        stalls = Duration::ZERO;
                        last_poll = now;
                        if this.update(cx, |this, _| this.pressure.pause()).is_err() {
                            break;
                        }
                        continue;
                    }
                    if late > Duration::from_millis(100) {
                        stalls += late;
                    }
                    let elapsed = now.duration_since(last_poll);
                    if elapsed < POLL {
                        continue;
                    }
                    let stall_share = (stalls.as_secs_f64() / elapsed.as_secs_f64()).min(1.);
                    stalls = Duration::ZERO;
                    last_poll = now;
                    let usage = cx
                        .background_executor()
                        .spawn(async { resources::usage() })
                        .await;
                    last_tick = Instant::now();
                    let sample = Sample { usage, stall_share };
                    if this
                        .update(cx, |this, cx| this.observe(sample, now, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        Self {
            pressure: Pressure::default(),
            _sampler: sampler,
        }
    }

    pub(super) fn observe(&mut self, sample: Sample, now: Instant, cx: &mut Context<Self>) {
        let before = (self.pressure.active, self.pressure.reasons.clone());
        self.pressure.sample(sample, now);
        if before != (self.pressure.active, self.pressure.reasons.clone()) {
            cx.notify();
        }
    }
}
