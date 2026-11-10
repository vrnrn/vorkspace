//! Ordered settings publication and its single background flush.
use super::*;

#[derive(Default)]
pub(super) struct SettingsWrites {
    pending: Mutex<HashMap<&'static str, Arc<str>>>,
}

impl SettingsWrites {
    fn stage<const N: usize>(&self, values: [(&'static str, String); N]) {
        self.pending
            .lock()
            .unwrap()
            .extend(values.into_iter().map(|(key, value)| (key, value.into())));
    }

    fn has_pending(&self) -> bool {
        !self.pending.lock().unwrap().is_empty()
    }

    pub(super) fn commit(
        &self,
        store: &Database,
        values: &[(&'static str, &str)],
    ) -> Result<(), String> {
        let store = locked(store)?;
        store.save_settings(values)?;
        // The store lock orders this commit after any older queued flush.
        let mut pending = self.pending.lock().unwrap();
        for (key, _) in values {
            pending.remove(key);
        }
        Ok(())
    }

    pub(super) fn flush(&self, store: &Database) -> Result<(), String> {
        // Serialize writes through the store and pick the latest snapshot
        // only after acquiring it, including during app shutdown.
        let store = locked(store)?;
        let snapshot = self.pending.lock().unwrap().clone();
        if !snapshot.is_empty() {
            let values: Vec<_> = snapshot
                .iter()
                .map(|(&key, value)| (key, value.as_ref()))
                .collect();
            store.save_settings(&values)?;
            let mut pending = self.pending.lock().unwrap();
            pending.retain(|key, current| {
                !snapshot
                    .get(key)
                    .is_some_and(|saved| Arc::ptr_eq(current, saved))
            });
        }
        Ok(())
    }
}

impl AppCore {
    pub(in crate::shell) fn flush_pending(&self, cx: &App) -> gpui::Task<Result<(), String>> {
        let writes = self.writes.clone();
        let store = self.store.clone();
        cx.background_executor()
            .spawn(async move { writes.flush(&store) })
    }

    pub(super) fn stage_settings<const N: usize>(
        &mut self,
        values: [(&'static str, String); N],
        cx: &mut Context<Self>,
    ) {
        self.writes.stage(values);
        self.flush_settings(cx);
    }

    fn flush_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_flush.is_some() {
            self.settings_flush_again = true;
            return;
        }
        let work = self.flush_pending(cx);
        self.settings_flush = Some(cx.spawn(async move |core, cx| {
            let result = work.await;
            let _ = core.update(cx, |core, cx| {
                core.settings_flush = None;
                if let Err(error) = result {
                    for window in &core.windows {
                        let _ = window.update(cx, |shell, cx| {
                            shell.error =
                                Some(format!("Could not save workspace settings: {error}"));
                            cx.notify();
                        });
                    }
                }
                if std::mem::take(&mut core.settings_flush_again) && core.writes.has_pending() {
                    core.flush_settings(cx);
                }
            });
        }));
    }
}
