//! Ordered settings publication and its single background flush.
use super::*;

#[derive(Default)]
pub(super) struct SettingsWrites {
    pending: Mutex<Pending>,
}

#[derive(Default)]
struct Pending {
    values: HashMap<&'static str, Arc<str>>,
    layouts: Option<Arc<WindowLayouts>>,
}

impl Pending {
    fn matches_layout(&self, layout: Option<&Arc<WindowLayouts>>) -> bool {
        self.layouts.as_ref().map(Arc::as_ptr) == layout.map(Arc::as_ptr)
    }
}

impl SettingsWrites {
    fn stage<const N: usize>(&self, values: [(&'static str, String); N]) {
        self.pending
            .lock()
            .unwrap()
            .values
            .extend(values.into_iter().map(|(key, value)| (key, value.into())));
    }

    pub(super) fn stage_layouts(&self, layouts: WindowLayouts) {
        self.pending.lock().unwrap().layouts = Some(Arc::new(layouts));
    }

    fn has_pending(&self) -> bool {
        let pending = self.pending.lock().unwrap();
        pending.layouts.is_some() || !pending.values.is_empty()
    }

    pub(super) fn commit_layouts(
        &self,
        store: &Database,
        layouts: &WindowLayouts,
    ) -> Result<(), String> {
        let encoded = layouts.settings_values();
        let values = encoded
            .each_ref()
            .map(|(key, value)| (*key, value.as_str()));
        let store = locked(store)?;
        store.save_settings(&values)?;
        self.pending.lock().unwrap().layouts = None;
        Ok(())
    }

    pub(super) fn flush(&self, store: &Database) -> Result<(), String> {
        self.flush_encoding(store, WindowLayouts::settings_values)
    }

    fn flush_encoding(
        &self,
        store: &Database,
        encode: impl Fn(&WindowLayouts) -> [(&'static str, String); 2],
    ) -> Result<(), String> {
        loop {
            let layouts = self.pending.lock().unwrap().layouts.clone();
            let encoded = layouts.as_deref().map(&encode);
            let store = locked(store)?;
            let snapshot = {
                let pending = self.pending.lock().unwrap();
                // A newer stage or synchronous commit may have won while
                // encoding. Recheck under the lock that orders publication.
                if !pending.matches_layout(layouts.as_ref()) {
                    continue;
                }
                pending.values.clone()
            };
            let mut values: Vec<_> = snapshot
                .iter()
                .map(|(&key, value)| (key, value.as_ref()))
                .collect();
            if let Some(encoded) = &encoded {
                values.extend(encoded.iter().map(|(key, value)| (*key, value.as_str())));
            }
            if values.is_empty() {
                return Ok(());
            }
            store.save_settings(&values)?;
            let mut pending = self.pending.lock().unwrap();
            pending.values.retain(|key, current| {
                !snapshot
                    .get(key)
                    .is_some_and(|saved| Arc::ptr_eq(current, saved))
            });
            if pending.matches_layout(layouts.as_ref()) {
                pending.layouts = None;
            }
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layouts(project: &str) -> WindowLayouts {
        let mut layouts = WindowLayouts::default();
        let workspace = Workspace {
            project: project.into(),
            ..Default::default()
        };
        layouts.record(1, serde_json::to_string(&workspace).unwrap(), true);
        layouts
    }

    #[test]
    fn an_encoding_in_flight_cannot_replace_a_newer_durable_layout() {
        let database = Arc::new(Mutex::new(
            vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let writes = SettingsWrites::default();
        writes.stage_layouts(layouts("/old"));
        writes.stage([("vorkspaces", "pending definitions".into())]);
        writes
            .flush_encoding(&database, |old| {
                let encoded = old.settings_values();
                writes.commit_layouts(&database, &layouts("/new")).unwrap();
                encoded
            })
            .unwrap();
        let store = locked(&database).unwrap();
        let legacy = store.ui_state().unwrap().unwrap();
        let workspace: Workspace = serde_json::from_str(&legacy).unwrap();
        assert_eq!(workspace.project, "/new");
        let saved = WindowLayouts::load(&store);
        let workspace: Workspace = serde_json::from_str(&saved.entries[0].workspace).unwrap();
        assert_eq!(workspace.project, "/new");
        assert_eq!(
            store.setting("vorkspaces").unwrap().as_deref(),
            Some("pending definitions")
        );
        assert!(!writes.has_pending());
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

    pub(super) fn flush_settings(&mut self, cx: &mut Context<Self>) {
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
