//! Scheduler ownership and durable connector retries across window handoffs.
use super::*;

impl AppCore {
    /// The requesting window takes a vacant or stale scheduler lease.
    fn claim_scheduler(&mut self, shell: &WeakEntity<Shell>) -> bool {
        match &self.scheduler {
            Some(holder) if holder.upgrade().is_some() => holder == shell,
            _ => {
                self.scheduler = Some(shell.clone());
                true
            }
        }
    }

    pub(in crate::shell) fn automation_events(&self) -> Vec<vorkspace_core::inbox::Item> {
        self.automation_events.values().cloned().collect()
    }

    pub(in crate::shell) fn queue_automation_events(
        &mut self,
        items: Vec<vorkspace_core::inbox::Item>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for item in items {
            let key = item.key();
            let current = self.automation_events.get(&key);
            if current.is_none_or(|old| old != &item && old.updated_ms() <= item.updated_ms()) {
                self.automation_events.insert(key, item);
                changed = true;
            }
        }
        if !changed {
            return;
        }
        if self.automation_events.len() > 500 {
            let mut items = self.automation_events();
            items.sort_unstable_by_key(|item| std::cmp::Reverse(item.updated_ms()));
            self.automation_events = items
                .into_iter()
                .take(500)
                .map(|item| (item.key(), item))
                .collect();
        }
        self.stage_automation_events(cx);
    }

    pub(in crate::shell) fn settle_automation_events(
        &mut self,
        attempted: Vec<vorkspace_core::inbox::Item>,
        retry: Vec<vorkspace_core::inbox::Item>,
        cx: &mut Context<Self>,
    ) {
        let mut retry: HashMap<_, _> = retry.into_iter().map(|item| (item.key(), item)).collect();
        let mut changed = false;
        for item in attempted {
            let key = item.key();
            // A newer connector refresh may have arrived during the claim.
            if self.automation_events.get(&key) != Some(&item) {
                continue;
            }
            match retry.remove(&key) {
                Some(pending) if pending != item => {
                    self.automation_events.insert(key, pending);
                    changed = true;
                }
                Some(_) => {}
                None => {
                    self.automation_events.remove(&key);
                    changed = true;
                }
            }
        }
        if changed {
            self.stage_automation_events(cx);
        }
    }

    fn stage_automation_events(&mut self, cx: &mut Context<Self>) {
        let snapshot =
            serde_json::to_string(&self.automation_events()).expect("serialize connector retries");
        self.stage_settings([(AUTOMATION_EVENTS_KEY, snapshot)], cx);
    }
}

impl Shell {
    pub(in crate::shell) fn holds_scheduler(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(core) = self.app_core.clone() else {
            return true;
        };
        let me = cx.weak_entity();
        core.update(cx, |core, _| core.claim_scheduler(&me))
    }
}
