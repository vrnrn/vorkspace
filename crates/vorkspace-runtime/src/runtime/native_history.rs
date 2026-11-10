//! Reconcile native history off the UI thread, then publish through the Store.
use super::*;
use std::path::Path;
use vorkspace_core::store::{NativeHistoryRequest, NativeHistoryResult, Store};
#[cfg(test)]
mod tests;

pub fn sync(store: &Database, session: i64, root: &Path) -> Result<NativeHistoryResult, String> {
    sync_checked(store, session, root).map(|(result, _)| result)
}

fn sync_checked(
    store: &Database,
    session: i64,
    root: &Path,
) -> Result<(NativeHistoryResult, Option<NativeHistoryRequest>), String> {
    let Some(mut request) = locked(store)?.native_history_request(session, root)? else {
        return Ok((NativeHistoryResult::default(), None));
    };
    let batch = vorkspace_core::native_history::read(&request.binding, request.cursor.as_ref())?;
    // Keep admission behind both the durable cursor and cache invalidation.
    // Otherwise another turn could observe the new cursor but take the old
    // parked process between these two publications. Eviction itself does not
    // wait for process teardown; the provider owner retires it asynchronously.
    let mut owner = locked(store)?;
    let mut result = match batch {
        Some(batch) if batch.update.is_some() => {
            owner.import_native_history(&mut request, batch)?
        }
        batch => NativeHistoryResult {
            active: batch.is_some_and(|batch| batch.active),
            superseded: !owner.native_history_current(&request)?,
            ..Default::default()
        },
    };
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
    if result.active
        && !result.more
        && !result.superseded
        && let Some(token) = Store::native_history_recovery_token(&request)
    {
        result.active = !owner.native_history_acknowledged(&request, &token)?;
        result.recovery = result.active.then_some(token);
    }
    Ok((result, Some(request)))
}

pub fn recover(store: &Database, session: i64, root: &Path, expected: &str) -> Result<(), String> {
    let (result, request) = sync_checked(store, session, root)?;
    if result.superseded || result.more || result.recovery.as_deref() != Some(expected) {
        return Err("The native conversation changed. Refresh before recovering it.".into());
    }
    let owner = locked(store)?;
    let request = request.ok_or("The native conversation is no longer available")?;
    owner.acknowledge_native_history(&request, expected)?;
    provider::forget_session(
        Kind::from_id(&request.binding.provider),
        &request.binding.thread,
        &request.binding.account,
        Path::new(&request.session.cwd),
    );
    Ok(())
}

pub(super) fn before_turn(
    store: &Database,
    summary: &Session,
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<Option<NativeHistoryRequest>, String> {
    let Some(bot) = settings.bot.as_ref().filter(|_| !settings.text_only) else {
        return Ok(None);
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Stopped".into());
        }
        let (result, request) = sync_checked(store, summary.id, &bot.root)?;
        if result.superseded {
            return Err("The Bot conversation changed during native history sync. Retry to continue the same session.".into());
        }
        if result.more {
            continue;
        }
        if result.active {
            return Err("The external CLI has an unfinished turn. Stop or close it, then choose Resume here in the Bot chat.".into());
        }
        return Ok(request);
    }
}
