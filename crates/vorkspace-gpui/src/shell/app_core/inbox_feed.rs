//! Route discoveries from the store's connector feed.
use super::*;
use crate::inbox::{FeedEvent, InboxFeed, SessionLinks};
use vorkspace_core::inbox::NotificationProject;

impl AppCore {
    pub(in crate::shell) fn inbox_sources(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (Entity<SessionLinks>, Entity<InboxFeed>) {
        let feed = self.inbox_feed.get_or_insert_with(|| {
            let data_dir = self.data_dir.clone().unwrap_or_else(std::env::temp_dir);
            let feed = cx.new(|cx| InboxFeed::new(&self.store, data_dir, cx));
            cx.subscribe(&feed, Self::handle_inbox_feed).detach();
            feed
        });
        (self.inbox_links.clone(), feed.clone())
    }

    fn handle_inbox_feed(
        &mut self,
        feed: Entity<InboxFeed>,
        event: &FeedEvent,
        cx: &mut Context<Self>,
    ) {
        let FeedEvent::Discovered {
            projects,
            changed,
            appeared,
        } = event
        else {
            return;
        };
        if !projects.is_empty() {
            self.remember_preference(
                |prefs| {
                    merge_projects(&mut prefs.notification_projects, projects);
                },
                cx,
            );
        }
        let evaluate_automations = !appeared.is_empty();
        if evaluate_automations {
            self.queue_automation_events(appeared.clone(), cx);
        }
        if !evaluate_automations && changed.is_empty() {
            return;
        }
        let core = cx.weak_entity();
        let changed = changed.clone();
        cx.defer(move |cx| {
            let Some(core) = core.upgrade() else { return };
            let core = core.read(cx);
            let Some(window) = core
                .scheduler
                .as_ref()
                .and_then(WeakEntity::upgrade)
                .or_else(|| core.windows.iter().find_map(WeakEntity::upgrade))
            else {
                return;
            };
            let connectors = feed.read(cx).connectors(cx);
            window.update(cx, |shell, cx| {
                shell.chime_inbox_activity(&connectors, &changed, cx);
                if evaluate_automations {
                    shell.drain_inbox_automation_events(cx);
                }
            });
        });
    }
}

fn merge_projects(catalog: &mut serde_json::Value, projects: &[NotificationProject]) {
    if !catalog.is_array() {
        *catalog = serde_json::Value::Array(Vec::new());
    }
    let entries = catalog.as_array_mut().expect("notification catalog array");
    for project in projects {
        let mut value = serde_json::to_value(project).expect("serialize notification project");
        if let Some(old) = entries
            .iter_mut()
            .find(|entry| entry["id"].as_str() == Some(&project.id))
        {
            let mut paths = old["paths"].as_array().cloned().unwrap_or_default();
            for path in &project.paths {
                let path = serde_json::Value::String(path.clone());
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
            value["paths"] = serde_json::Value::Array(paths);
            *old = value;
        } else {
            entries.push(value);
        }
    }
}
