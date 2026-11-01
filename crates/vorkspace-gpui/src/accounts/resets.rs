//! Banked Codex resets are shared across footers so only one claim runs per account.
use super::*;

#[derive(Clone)]
pub(crate) struct ResetRow {
    pub key: String,
    pub credit: Option<usage::ResetCredit>,
    pub title: String,
}

pub(crate) fn rows(snapshot: Option<&Snapshot>) -> Vec<ResetRow> {
    let Some(credits) = snapshot.and_then(|s| s.reset_credits.as_ref()) else {
        return Vec::new();
    };
    if credits.available_count == 0 {
        return Vec::new();
    }
    let mut rows: Vec<_> = credits
        .credits
        .iter()
        .filter(|c| matches!(c.status.as_str(), "available" | "unknown"))
        .enumerate()
        .map(|(i, credit)| ResetRow {
            key: format!("credit:{}", credit.id),
            credit: Some(credit.clone()),
            title: credit
                .title
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("Banked reset {}", i + 1)),
        })
        .collect();
    for i in rows.len()..credits.available_count as usize {
        rows.push(ResetRow {
            key: format!("unlisted:{i}"),
            credit: None,
            title: format!("Banked reset {}", i + 1),
        });
    }
    rows
}

#[derive(Clone)]
pub(crate) struct ResetAction {
    /// Retained after a failed refresh to prevent spending a stale card again.
    pub row: Option<String>,
    pub result: Option<Result<String, String>>,
}
impl ResetAction {
    pub(crate) fn applying(&self) -> bool {
        self.result.is_none()
    }
    pub(crate) fn message(&self) -> Option<&str> {
        Some(match self.result.as_ref()? {
            Ok(outcome) => match outcome.as_str() {
                "reset" => "Codex usage was reset.",
                "nothingToReset" => "There’s no active usage to reset.",
                "noCredit" => "No banked resets are available.",
                "alreadyRedeemed" => "That reset was already used.",
                _ => "Codex returned an unknown reset result.",
            },
            Err(error) => error,
        })
    }
}

impl Accounts {
    pub(crate) fn consume_reset(&mut self, key: Key, row: &str, cx: &mut Context<Self>) {
        self.consume_reset_with(
            key,
            row,
            |profile, credit| {
                let result = usage::consume_reset(profile, credit);
                // Read even after errors: the server may have applied the claim.
                let refreshed = usage::fetch_for(Kind::Codex, profile, None);
                (result, refreshed)
            },
            cx,
        );
    }

    fn consume_reset_with(
        &mut self,
        key: Key,
        row: &str,
        consume: impl FnOnce(
            &Profile,
            Option<&str>,
        ) -> (Result<String, String>, Result<Snapshot, FetchError>)
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        if key.kind != Kind::Codex
            || (key.profile.id != accounts::DEFAULT && !self.book.exists(key.kind, &key.profile.id))
            || self.reset_actions.get(&key).is_some_and(|a| {
                a.applying()
                    || (a.row.as_deref() == Some(row)
                        && a.result.as_ref().is_some_and(Result::is_ok))
            })
        {
            return;
        }
        let Some(row) = rows(self.usage.get(&key).and_then(|s| s.snapshot.as_ref()))
            .into_iter()
            .find(|r| r.key == row)
        else {
            return;
        };
        let state = self.usage.entry(key.clone()).or_default();
        // Older usage reads must not overwrite the post-claim snapshot.
        state.generation += 1;
        state.fetching = true;
        let generation = state.generation;
        self.reset_actions.insert(
            key.clone(),
            ResetAction {
                row: Some(row.key),
                result: None,
            },
        );
        let profile = key.profile.clone();
        let work = cx
            .background_executor()
            .spawn(async move { consume(&profile, row.credit.as_ref().map(|c| c.id.as_str())) });
        cx.spawn(async move |this, cx| {
            let (result, refreshed) = work.await;
            let _ = this.update(cx, |this, cx| {
                let Some(state) = this.usage.get_mut(&key) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                let refreshed_ok = refreshed.is_ok();
                let queued = std::mem::take(&mut state.queued);
                state.attempted = Some(Instant::now());
                state.complete(refreshed);
                if let Some(identity) = state.snapshot.as_ref().and_then(|s| s.identity.clone()) {
                    this.identities
                        .insert((key.kind, key.profile.id.clone()), identity);
                }
                if let Some(action) = this.reset_actions.get_mut(&key) {
                    action.result = Some(result);
                    if refreshed_ok {
                        action.row = None;
                    }
                }
                if queued {
                    this.refresh(key, true, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
