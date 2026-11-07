//! UI batches stay responsive while the shared JSON transcript is saved at a
//! bounded cadence. Delivery receipts and settled turns always flush immediately.
use super::*;
use std::time::Instant;

const SHARED_SAVE_INTERVAL: Duration = Duration::from_millis(250);

pub(super) struct PendingSave {
    from: Option<usize>,
    context_dirty: bool,
    last_flush: Instant,
    interval: Duration,
    context_owner: Option<(String, String)>,
    /// Only Bots bind usage to a specific native thread, including detachment.
    bot_context_owner: Option<Option<String>>,
    pub(super) writer: Option<vorkspace_core::store::TurnHistory>,
    pub(super) overlap: bool,
}

impl PendingSave {
    pub(super) fn new(shared: bool, now: Instant) -> Self {
        Self {
            from: None,
            context_dirty: false,
            last_flush: now,
            interval: if shared {
                SHARED_SAVE_INTERVAL
            } else {
                Duration::ZERO
            },
            context_owner: None,
            bot_context_owner: None,
            writer: None,
            overlap: false,
        }
    }

    pub(super) fn own_context(&mut self, provider: &str, account: &str) {
        self.context_owner = Some((provider.into(), account.into()));
    }

    pub(super) fn own_bot_context(&mut self, provider_id: Option<&str>) {
        self.bot_context_owner = Some(provider_id.map(str::to_owned));
    }

    pub(super) fn changed(&mut self, row: usize) {
        self.from = Some(self.from.map_or(row, |old| old.min(row)));
    }

    pub(super) fn context_changed(&mut self) {
        self.context_dirty = true;
    }

    pub(super) fn due(&self, boundary: bool, now: Instant) -> bool {
        (self.from.is_some() || self.context_dirty)
            && (boundary || now.saturating_duration_since(self.last_flush) >= self.interval)
    }

    pub(super) fn flush(
        &mut self,
        store: &Database,
        session: i64,
        transcript: &Transcript,
        context: &vorkspace_core::context::Usage,
        now: Instant,
    ) -> Result<(), String> {
        if self.from.is_none() && !self.context_dirty {
            return Ok(());
        }
        let mut store = locked(store)?;
        if let Some(from) = self.from {
            self.write(&mut store, session, from, &transcript.rows[from..], None)?;
        }
        if self.context_dirty {
            if let Some((provider, account)) = &self.context_owner {
                if let Some(native_id) = &self.bot_context_owner {
                    store.save_bot_context_usage_if_owner(
                        session,
                        context,
                        provider,
                        account,
                        native_id.as_deref(),
                    )?;
                } else {
                    store.save_context_usage_if_owner(session, context, provider, account)?;
                }
            } else {
                store.save_context_usage(session, context)?;
            }
        }
        // Keep pending changes after a failed write so final cleanup retries.
        self.from = None;
        self.context_dirty = false;
        self.last_flush = now;
        Ok(())
    }

    pub(super) fn write(
        &mut self,
        store: &mut Store,
        session: i64,
        from: usize,
        rows: &[Row],
        message: Option<&str>,
    ) -> Result<(), String> {
        if !self.overlap {
            self.overlap =
                !store.write_turn_rows(session, from, rows, message, self.writer.as_mut())?;
        }
        if self.overlap {
            return Err("The saved conversation changed during this turn".into());
        }
        Ok(())
    }

    pub(super) fn receipt(
        &mut self,
        store: &Database,
        session: i64,
        at: usize,
        info: vorkspace_core::agent::TurnInfo,
    ) -> Result<(), String> {
        if let Some(writer) = self.writer.as_mut() {
            self.overlap = !locked(store)?.save_turn_info(session, at, info, writer)?;
            if self.overlap {
                return Err("The saved conversation changed during this turn".into());
            }
            Ok(())
        } else {
            save_turn_receipt(store, session, at, info)
        }
    }

    pub(super) fn accept(
        &mut self,
        store: &Database,
        session: i64,
        from: usize,
        transcript: &Transcript,
        message: &str,
        now: Instant,
    ) -> Result<(), String> {
        let from = self.from.map_or(from, |old| old.min(from));
        self.write(
            &mut *locked(store)?,
            session,
            from,
            &transcript.rows[from..],
            Some(message),
        )?;
        self.from = None;
        self.last_flush = now;
        Ok(())
    }
}

pub(super) fn save_turn_receipt(
    store: &Database,
    session: i64,
    turn: usize,
    info: vorkspace_core::agent::TurnInfo,
) -> Result<(), String> {
    let store = locked(store)?;
    // The picker/composer may have changed during the run. Update only its
    // immutable turn receipts, using the latest settings revision.
    let mut settings = store.agent_settings(session)?.unwrap_or_default();
    settings.turns.insert(turn, info);
    store.save_agent_settings(session, &settings)
}

#[cfg(test)]
mod tests;
