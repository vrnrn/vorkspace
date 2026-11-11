//! Scheduler ownership and durable connector retries across window handoffs.
use super::super::automation_runs::ServiceHandle;
use super::*;

pub(super) struct AutomationServiceReady(pub Result<ServiceHandle, String>);
impl EventEmitter<AutomationServiceReady> for AppCore {}

impl AppCore {
    pub(in crate::shell) fn request_automation_service(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<ServiceHandle> {
        if let Some(service) = &self.automation_service {
            return Some(service.clone());
        }
        if self.automation_opening.is_none() {
            let store = self.store.clone();
            let work = cx.background_executor().spawn(async move {
                let path = locked(&store)?.database_path().to_path_buf();
                let service = vorkspace_core::automations::Service::open(&path)?;
                Ok::<_, String>(Arc::new(Mutex::new(service)))
            });
            self.automation_opening = Some(cx.spawn(async move |core, cx| {
                let result = work.await;
                let _ = core.update(cx, |core, cx| {
                    core.automation_opening = None;
                    core.automation_service = result.as_ref().ok().cloned();
                    cx.emit(AutomationServiceReady(result));
                });
            }));
        }
        None
    }

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
