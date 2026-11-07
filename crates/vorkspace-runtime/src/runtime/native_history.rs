//! Reconcile native history off the UI thread, then publish through the Store.
use super::*;
use std::path::Path;
use vorkspace_core::store::NativeHistoryResult;

pub fn sync(store: &Database, session: i64, root: &Path) -> Result<NativeHistoryResult, String> {
    let Some(request) = locked(store)?.native_history_request(session, root)? else {
        return Ok(NativeHistoryResult::default());
    };
    let Some(batch) =
        vorkspace_core::native_history::read(&request.binding, request.cursor.as_ref())?
    else {
        return Ok(NativeHistoryResult::default());
    };
    if !batch.changed {
        return Ok(NativeHistoryResult {
            active: batch.active,
            ..Default::default()
        });
    }
    // Keep admission behind both the durable cursor and cache invalidation.
    // Otherwise another turn could observe the new cursor but take the old
    // parked process between these two publications. Eviction itself does not
    // wait for process teardown; the provider owner retires it asynchronously.
    let mut owner = locked(store)?;
    let result = owner.import_native_history(&request, batch)?;
    if result.reload_provider && !result.superseded {
        // An idle process may retain a context predating CLI messages or native
        // compaction. Resume the same ID from disk on the next managed turn.
        provider::forget_session(
            Kind::from_id(&request.binding.provider),
            &request.binding.thread,
            &request.binding.account,
            Path::new(&request.session.cwd),
        );
    }
    Ok(result)
}

pub(super) fn before_turn(
    store: &Database,
    summary: &Session,
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let Some(bot) = settings.bot.as_ref().filter(|_| !settings.text_only) else {
        return Ok(());
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        let result = sync(store, summary.id, &bot.root)?;
        if result.superseded {
            return Err("The Bot conversation changed during native history sync. Retry to continue the same session.".into());
        }
        if result.more {
            continue;
        }
        if result.active {
            return Err("This Bot is still working in the external CLI. Finish or stop that turn before sending here.".into());
        }
        return Ok(());
    }
}
