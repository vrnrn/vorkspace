//! Filesystem signals wake the shared checkout while it has an active viewer.
use super::{Checkout, FULL_REFRESH, POLL, Refresh};
use gpui::{AsyncApp, WeakEntity};
use std::{future::poll_fn, path::Path, pin::pin, task::Poll, time::Instant};
use vorkspace_macos::filesystem_events::{Change, Watch};

pub(super) async fn observe(checkout: &WeakEntity<Checkout>, path: String, cx: &mut AsyncApp) {
    let watch = cx
        .background_executor()
        .spawn(async move {
            let paths = vorkspace_core::scm::checkout_watch_paths(Path::new(&path))?;
            Watch::start(&paths)
        })
        .await;
    let Some(watch) = watch else { return };
    // Re-read once after admission to bridge the initial scan and stream start.
    if checkout
        .update(cx, |checkout, cx| checkout.scan(Refresh::Probe, cx))
        .is_err()
    {
        return;
    }
    let mut next_probe = Instant::now() + POLL;
    let mut reconcile = Instant::now() + FULL_REFRESH;
    loop {
        let mut tick = pin!(
            cx.background_executor()
                .timer(reconcile.saturating_duration_since(Instant::now()))
        );
        let mut event = pin!(watch.changed());
        let change = poll_fn(|cx| {
            if tick.as_mut().poll(cx).is_ready() {
                Poll::Ready(Change::Reconcile)
            } else {
                event.as_mut().poll(cx)
            }
        })
        .await;
        if change == Change::Files {
            // Bound a sustained burst to the old poll rate, while the first
            // event in a quiet checkout can publish immediately.
            let delay = next_probe
                .min(reconcile)
                .saturating_duration_since(Instant::now());
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
        }
        let change = change.max(watch.pending());
        let now = Instant::now();
        let refresh = if change != Change::Files || now >= reconcile {
            reconcile = now + FULL_REFRESH;
            Refresh::Reconcile
        } else {
            Refresh::Probe
        };
        next_probe = now + POLL;
        if checkout
            .update(cx, |checkout, cx| checkout.scan(refresh, cx))
            .is_err()
            || change == Change::Reopen
        {
            // The normal poll path remains available if a watched root moves.
            return;
        }
    }
}
