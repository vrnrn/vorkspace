//! The application owns the linked feed and acknowledgment writes.
use super::*;
use crate::notifications::github_activity::{
    self, GithubActivity, GithubActivityEvent, LinkedSession,
};
use crate::notifications::{
    delivery::{Permission, Target},
    platform,
};

impl AppCore {
    pub(in crate::shell) fn start_native_notifications(&mut self, cx: &mut Context<Self>) {
        if self.notification_clicks.is_some() || !platform::initialize() {
            return;
        }
        if self.notification_permission == Permission::Unsupported {
            self.notification_permission = Permission::Prompt;
        }
        self.refresh_notification_permission(cx);
        let clicks = platform::clicks();
        self.notification_clicks = Some(cx.spawn(async move |core, cx| {
            while let Ok(target) = clicks.recv().await {
                let id = match target {
                    Target::Session(id) | Target::Reminder { session_id: id, .. } => id,
                };
                let routed = cx.update(|cx| {
                    let Some(core) = core.upgrade() else {
                        return false;
                    };
                    let Some(shell) = core.read(cx).route(id, cx) else {
                        return false;
                    };
                    shell.update(cx, |shell, cx| shell.open_notice_target(target, cx));
                    true
                });
                if !routed {
                    // Keep a click received during window teardown for the
                    // next workspace window.
                    platform::queue_click(target);
                    break;
                }
            }
        }));
    }

    pub(in crate::shell) fn set_notification_permission(&mut self, permission: Permission) {
        self.notification_permission_probe = None;
        self.notification_permission = permission;
    }

    pub(in crate::shell) fn refresh_notification_permission(&mut self, cx: &mut Context<Self>) {
        if self.notification_permission_probe.is_some() {
            return;
        }
        let result = platform::permission();
        self.notification_permission_probe = Some(cx.spawn(async move |core, cx| {
            let result = result.recv().await;
            let _ = core.update(cx, |core, _| {
                core.notification_permission_probe = None;
                if let Ok(permission) = result {
                    core.notification_permission = permission;
                }
            });
        }));
    }

    pub(super) fn observe_linked_activity(feed: &Entity<GithubActivity>, cx: &mut Context<Self>) {
        cx.subscribe(feed, |core, feed, event, cx| match event {
            GithubActivityEvent::Acknowledged(encoded) => {
                core.stage_settings([(github_activity::SEEN_KEY, encoded.clone())], cx);
            }
            GithubActivityEvent::Changed { new_activity } => {
                if new_activity.is_empty() {
                    return;
                }
                let Some(window) = core
                    .scheduler
                    .as_ref()
                    .and_then(WeakEntity::upgrade)
                    .or_else(|| core.windows.iter().find_map(WeakEntity::upgrade))
                else {
                    return;
                };
                let shell = window.read(cx);
                if new_activity
                    .iter()
                    .filter_map(|id| feed.read(cx).cards.get(id))
                    .any(|card| {
                        shell.should_chime_linked_activity(card, core.linked_activity_started_at)
                    })
                {
                    crate::sound::play(crate::sound::Cue::LinkedActivity, core.store.clone());
                }
            }
        })
        .detach();
    }

    pub(in crate::shell) fn start_linked_activity(
        &mut self,
        metadata: Option<serde_json::Value>,
        cx: &mut Context<Self>,
    ) {
        if self.linked_activity_clock.is_some() {
            return;
        }
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store).and_then(|store| {
                github_activity::load_seen_with_metadata(&store, metadata.as_ref())
            })
        });
        self.linked_activity_clock = Some(cx.spawn(async move |core, cx| {
            let seen = work.await.unwrap_or_default();
            if core
                .update(cx, |core, cx| {
                    core.github_activity
                        .update(cx, |feed, _| feed.restore_seen(seen));
                })
                .is_err()
            {
                return;
            }
            loop {
                if core
                    .update(cx, |core, cx| core.poll_linked_activity(cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(60))
                    .await;
            }
        }));
    }

    fn linked_sessions(&self, cx: &App) -> HashMap<i64, LinkedSession> {
        let windows: Vec<_> = self
            .windows
            .iter()
            .filter_map(WeakEntity::upgrade)
            .collect();
        let mut linked: HashMap<_, _> = self
            .directory
            .read(cx)
            .rows
            .values()
            .filter_map(|record| LinkedSession::from_summary(&record.summary))
            .map(|session| (session.id, session))
            .collect();
        for window in windows {
            for view in window.read(cx).notification_chats(cx) {
                let summary = &view.read(cx).summary;
                if let Some(session) = LinkedSession::from_summary(summary) {
                    linked.insert(session.id, session);
                } else {
                    linked.remove(&summary.id);
                }
            }
        }
        linked
    }

    fn poll_linked_activity(&mut self, cx: &mut Context<Self>) {
        let Some(seen) = self.github_activity.update(cx, |feed, _| feed.begin_poll()) else {
            return;
        };
        let sessions = self.linked_sessions(cx);
        if sessions.is_empty() {
            self.github_activity.update(cx, |feed, cx| {
                feed.finish_poll(HashMap::new(), &sessions, cx);
            });
            return;
        }
        let work = cx
            .background_executor()
            .spawn(async move { github_activity::fetch(sessions, &seen) });
        cx.spawn(async move |core, cx| {
            let cards = work.await;
            let _ = core.update(cx, |core, cx| {
                let current = core.linked_sessions(cx);
                core.github_activity
                    .update(cx, |feed, cx| feed.finish_poll(cards, &current, cx));
            });
        })
        .detach();
    }
}
