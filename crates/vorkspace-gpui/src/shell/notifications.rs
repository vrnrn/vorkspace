//! Delivers window notifications and routes native clicks back to their session.
use super::*;
use crate::notifications::{
    app_update::{self, AppUpdateEvent, InstalledUpdateNotice, WhatsNew, WhatsNewEvent},
    approvals::{self, ApprovalNotices, SessionInputs},
    delivery::{self, Permission, Target},
    errors,
    file_error::{FileActionError, FileActionErrorEvent},
    github_activity::{self, GithubActivity, GithubActivityEvent},
    harness_updates::{HarnessUpdates, HarnessUpdatesEvent},
    linked_activity::{LinkedActivity, LinkedActivityEvent},
    platform, reminders,
};
use crate::settings::ProjectNotifications;
use std::time::Duration;

pub(super) struct State {
    pub region: AnchorSlot,
    pub harness: Entity<HarnessUpdates>,
    pub approvals: Entity<ApprovalNotices>,
    pub reminders: Entity<reminders::View>,
    pub installed: Entity<InstalledUpdateNotice>,
    pub file_error: Entity<FileActionError>,
    pub github: Entity<GithubActivity>,
    linked: HashMap<i64, Entity<LinkedActivity>>,
    linked_cleanup: HashSet<i64>,
    pub(super) whats_new: Option<(Entity<WhatsNew>, Subscription, FocusHandle)>,
    announcements: approvals::InputAnnouncements,
    #[cfg(test)]
    spoken: Vec<String>,
    responding: HashSet<i64>,
    sequence: u64,
    badge: usize,
    sounds_enabled: bool,
    sounds_enabled_at: u64,
    mute_expiry: Option<(u64, gpui::Task<()>)>,
}

impl State {
    pub(super) fn new(
        workspace: &Workspace,
        github: Entity<GithubActivity>,
        cx: &mut Context<Shell>,
    ) -> Self {
        Self {
            region: AnchorSlot::default(),
            harness: cx
                .new(|cx| HarnessUpdates::new(workspace.preferences.hidden_providers.clone(), cx)),
            approvals: cx.new(|_| ApprovalNotices::default()),
            reminders: cx.new(|_| reminders::View::default()),
            installed: cx.new(|_| InstalledUpdateNotice::new(None)),
            file_error: cx.new(|_| FileActionError::new()),
            github,
            linked: HashMap::new(),
            linked_cleanup: HashSet::new(),
            whats_new: None,
            announcements: Default::default(),
            #[cfg(test)]
            spoken: Vec::new(),
            responding: HashSet::new(),
            sequence: 0,
            badge: 0,
            sounds_enabled: workspace.preferences.sounds,
            sounds_enabled_at: 0,
            mute_expiry: None,
        }
    }
}

impl Shell {
    pub(super) fn inbox_activity(
        &mut self,
        projects: &[vorkspace_core::inbox::NotificationProject],
        changed: &[vorkspace_core::inbox::Item],
        cx: &mut Context<Self>,
    ) {
        let previous = &self.workspace.preferences.notification_projects;
        let mut catalog = previous.as_array().cloned().unwrap_or_default();
        for project in projects {
            let mut value = serde_json::to_value(project).unwrap();
            if let Some(old) = catalog
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
                catalog.push(value);
            }
        }
        let catalog = serde_json::Value::Array(catalog);
        if &catalog != previous {
            self.workspace.preferences.notification_projects = catalog;
            self.persist(cx);
            self.sync_settings(cx);
        }
        let connectors = self
            .inbox
            .as_ref()
            .map(|inbox| inbox.read(cx).connector_preferences())
            .unwrap_or_default();
        if self.holds_scheduler(cx)
            && crate::notifications::inbox::should_chime(
                &self.workspace.preferences,
                &connectors,
                changed,
                self.notification_state.sounds_enabled_at,
                vorkspace_core::time::now_ms(),
            )
        {
            crate::sound::play(crate::sound::Cue::LinkedActivity, self.store.clone());
        }
    }

    pub(super) fn refresh_notification_permission(&self, cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            core.update(cx, |core, cx| core.refresh_notification_permission(cx));
        }
    }

    /// Call from SessionChanged, so a turn beginning and ending between render
    /// frames still produces exactly one completion. Render sync shares the set.
    pub(super) fn notification_session_changed(&mut self, id: i64, cx: &mut Context<Self>) {
        self.process_notification_inputs(cx);
        let Some(view) = self.sessions.get(&id) else {
            return;
        };
        let chat = view.read(cx);
        if chat.responding() {
            self.notification_state.responding.insert(id);
            return;
        }
        if !self.notification_state.responding.remove(&id)
            || chat.busy
            || chat.notification_was_cancelled()
            || chat.summary.archived
            || self
                .bots
                .as_ref()
                .is_some_and(|bots| bots.read(cx).is_bot_session(id))
        {
            return;
        }
        let summary = chat.summary.clone();
        let body = chat.notification_reply_body(cx);
        self.announce(format!("{} finished", summary.title));
        self.deliver_session_notice(
            &summary,
            "agentFinished",
            &body,
            Some(crate::sound::Cue::Finished),
            cx,
        );
    }

    pub(super) fn bind_notices(&mut self, cx: &mut Context<Self>) {
        self.bind_linked_activity(cx);
        self.observations.push(cx.subscribe(
            &self.notification_state.harness,
            |this, _, event, cx| {
                if let HarnessUpdatesEvent::Updated(kind) = event {
                    for view in this.notification_chats(cx) {
                        if Kind::from_id(&view.read(cx).summary.provider) == *kind {
                            view.update(cx, |view, cx| {
                                view.refresh_models_after_harness_update(cx)
                            });
                        }
                    }
                    this.sync_settings(cx);
                }
                cx.notify();
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.approvals,
            |this, _, event, cx| match event {
                approvals::Action::OpenSession { session_id } => {
                    if let Ok(id) = session_id.parse() {
                        this.open_notice_target(Target::Session(id), cx);
                    }
                }
                approvals::Action::Decide {
                    session_id,
                    request_id,
                    decision,
                } => {
                    if let Ok(id) = session_id.parse()
                        && let Some(view) = this.notification_chat(id, cx)
                    {
                        view.update(cx, |view, cx| {
                            view.decide_notification_request(request_id, (**decision).clone(), cx);
                        });
                    }
                    cx.notify();
                }
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.reminders,
            |this, _, event, cx| match event {
                reminders::Action::Open { session_id, due_at } => this.open_notice_target(
                    Target::Reminder {
                        session_id: *session_id,
                        due_at: *due_at,
                    },
                    cx,
                ),
                reminders::Action::Dismiss { session_id, due_at } => {
                    this.clear_notification_reminder(*session_id, *due_at, cx)
                }
                reminders::Action::Snooze {
                    session_id,
                    expected_due_at,
                    due_at,
                } => this.snooze_notification_reminder(*session_id, *expected_due_at, *due_at, cx),
                reminders::Action::Retry => this.reload_notification_reminders(cx),
                reminders::Action::OpenSettings => {
                    cx.spawn(async move |this, cx| {
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.open_settings(crate::settings::Section::General, window, cx)
                        });
                    })
                    .detach();
                }
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.installed,
            |this, _, event, cx| match event {
                AppUpdateEvent::Open(version) => this.open_whats_new(version.clone(), cx),
                AppUpdateEvent::Dismiss => {
                    this.notification_state
                        .installed
                        .update(cx, |notice, cx| notice.set_version(None, cx));
                    this.invalidate_chrome(cx);
                    cx.notify();
                }
            },
        ));
        self.observations.push(cx.subscribe(
            &self.notification_state.file_error,
            |this, _, _: &FileActionErrorEvent, cx| {
                this.notification_state
                    .file_error
                    .update(cx, |notice, cx| notice.set_message(None, cx));
                cx.notify();
            },
        ));
        if let Some(bots) = self.bots.clone() {
            self.observations.push(cx.subscribe(
                &bots,
                |this, _, _: &crate::bots::NotificationChanged, cx| {
                    this.process_notification_inputs(cx);
                    cx.notify()
                },
            ));
            self.observations.push(cx.subscribe(
                &bots,
                |this, _, event: &crate::bots::NotificationFinished, cx| {
                    if !event.was_cancelled && !event.habit && !event.summary.archived {
                        this.deliver_session_notice(
                            &event.summary,
                            "agentFinished",
                            &event.body,
                            Some(crate::sound::Cue::Finished),
                            cx,
                        );
                    }
                },
            ));
        }
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            locked(&store).and_then(|store| app_update::consume_installed_update(&store))
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Some(version)) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.notification_state
                        .installed
                        .update(cx, |notice, cx| notice.set_version(Some(version), cx));
                    this.invalidate_chrome(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn start_native_notifications(&mut self, cx: &mut Context<Self>) {
        self.set_window_badge(0, cx);
        if let Some(core) = &self.app_core {
            core.update(cx, |core, cx| core.start_native_notifications(cx));
        }
        self.notification_state.badge = 0;
        self.process_notification_inputs(cx);
    }

    pub(super) fn notification_chats(&self, cx: &App) -> Vec<Entity<Conversation>> {
        let mut views: Vec<_> = self.sessions.values().cloned().collect();
        if let Some(bots) = &self.bots {
            views.extend(bots.read(cx).all_notification_chats(cx));
        }
        let mut seen = HashSet::new();
        views.retain(|view| seen.insert(view.read(cx).summary.id));
        views.sort_by_key(|view| view.read(cx).summary.id);
        views
    }

    fn notification_chat(&self, id: i64, cx: &App) -> Option<Entity<Conversation>> {
        self.sessions
            .get(&id)
            .cloned()
            .or_else(|| self.bots.as_ref()?.read(cx).notification_entity(id, cx))
    }

    fn announce(&mut self, message: String) {
        #[cfg(test)]
        self.notification_state.spoken.push(message.clone());
        vorkspace_macos::accessibility::announce(&message);
    }

    fn announce_input(&mut self, title: &str, label: &str) {
        self.announce(format!("{title} needs your input: {label}"));
    }

    /// Advance banners and the Dock badge while an occluded window skips render.
    fn process_notification_inputs(&mut self, cx: &mut Context<Self>) {
        let snapshots: Vec<_> = self
            .notification_chats(cx)
            .iter()
            .filter_map(|view| self.input_snapshot(view.read(cx), false, cx))
            .collect();
        self.notification_state.approvals.update(cx, |notices, _| {
            notices.track_pending(&snapshots);
        });
        self.publish_pending_inputs(&snapshots, cx);
    }

    fn input_snapshot(
        &self,
        chat: &Conversation,
        check_visibility: bool,
        cx: &App,
    ) -> Option<SessionInputs> {
        let pending = chat.pending_notification_inputs(cx);
        if pending.is_empty() {
            return None;
        }
        let id = chat.summary.id;
        Some(SessionInputs {
            session_id: id.to_string(),
            title: chat.display_title().to_owned(),
            provider: chat.summary.provider.clone(),
            harness: Kind::display_label(&chat.summary.provider).to_owned(),
            pending,
            inbox_ask: false,
            orchestration_worker: self.orchestration_worker(id),
            // An on-screen conversation already shows its request inline.
            conversation_focused: check_visibility && self.notice_session_visible(id, cx),
        })
    }

    fn publish_pending_inputs(&mut self, snapshots: &[SessionInputs], cx: &mut Context<Self>) {
        let badge = delivery::dock_badge_count(
            snapshots
                .iter()
                .filter_map(|s| Some((s.session_id.parse().ok()?, !s.pending.is_empty()))),
        );
        if badge != self.notification_state.badge {
            self.notification_state.badge = badge;
            self.set_window_badge(badge, cx);
        }
        for (session, input) in self.notification_state.announcements.reconcile(snapshots) {
            if let Ok(id) = session.session_id.parse()
                && let Some(view) = self.notification_chat(id, cx)
            {
                let summary = view.read(cx).summary.clone();
                self.announce_input(&summary.title, &input.label);
                let body = if input.kind == approvals::InputKind::Approval {
                    format!("Approve: {}", input.label)
                } else {
                    input.label
                };
                self.deliver_session_notice(
                    &summary,
                    "agentInput",
                    &body,
                    Some(crate::sound::Cue::Input),
                    cx,
                );
            }
        }
    }

    fn notice_session_visible(&self, id: i64, cx: &App) -> bool {
        if self.settings.is_some() {
            return false;
        }
        self.workspace.active_tab().is_some_and(|tab| {
            let layout = self.workspace.layout(tab);
            let split = layout.is_split();
            layout.leaves().into_iter().any(|leaf| {
                // A split can be scrolled or clipped. Only suppress a
                // toast when the whole pane can show the inline request.
                if split {
                    let Some((canvas, viewport)) =
                        self.pane_bounds.get().zip(self.pane_viewport.get())
                    else {
                        return false;
                    };
                    let rect = leaf.rect;
                    let top_left = viewport.origin
                        + self.pane_scroll.offset()
                        + gpui::point(canvas.size.width * rect.x, canvas.size.height * rect.y);
                    let bottom_right = top_left
                        + gpui::point(canvas.size.width * rect.w, canvas.size.height * rect.h);
                    let tolerance = px(0.5);
                    if top_left.x < viewport.left() - tolerance
                        || top_left.y < viewport.top() - tolerance
                        || bottom_right.x > viewport.right() + tolerance
                        || bottom_right.y > viewport.bottom() + tolerance
                    {
                        return false;
                    }
                }
                let pane = leaf.id;
                if self.workspace.session_for_pane(pane) == Some(id)
                    && !self.workspace.review_owners.contains_key(&pane)
                {
                    return !self.review_covers_chat(pane, cx);
                }
                let Some(crate::workspace::AppPane::Bot(bot)) = self
                    .workspace
                    .app_panes
                    .get(&pane)
                    .map(|pane| &pane.content)
                else {
                    return false;
                };
                self.workspace.preferences.bots
                    && !self.workspace.bot_hidden_in_vorkspace(bot)
                    && self.bots.as_ref().is_some_and(|manager| {
                        manager.read(cx).pane_notification_session(bot, cx) == Some(id)
                    })
            })
        })
    }

    pub(super) fn sync_notices(&mut self, cx: &mut Context<Self>) {
        let now = vorkspace_core::time::now_ms();
        let sounds = self.workspace.preferences.sounds;
        if self.notification_state.sounds_enabled != sounds {
            self.notification_state.sounds_enabled = sounds;
            if sounds {
                self.notification_state.sounds_enabled_at = now;
            }
        }
        let views = self.notification_chats(cx);
        let mut snapshots = Vec::new();
        let mut directories = HashMap::new();
        let mut completed = Vec::new();
        let mut live_ids = HashSet::new();
        for view in views {
            let chat = view.read(cx);
            let id = chat.summary.id;
            live_ids.insert(id);
            if chat.responding() {
                self.notification_state.responding.insert(id);
            } else if self.notification_state.responding.remove(&id)
                && !chat.busy
                && !chat.notification_was_cancelled()
                && !chat.summary.archived
                && self.sessions.contains_key(&id)
                && !self
                    .bots
                    .as_ref()
                    .is_some_and(|bots| bots.read(cx).is_bot_session(id))
            {
                completed.push((chat.summary.clone(), chat.notification_reply_body(cx)));
            }
            if let Some(snapshot) = self.input_snapshot(chat, self.window_active, cx) {
                directories.insert(snapshot.session_id.clone(), chat.summary.cwd.clone());
                snapshots.push(snapshot);
            }
        }
        self.notification_state
            .responding
            .retain(|id| live_ids.contains(id));
        self.publish_pending_inputs(&snapshots, cx);
        for (summary, body) in completed {
            self.announce(format!("{} finished", summary.title));
            self.deliver_session_notice(
                &summary,
                "agentFinished",
                &body,
                Some(crate::sound::Cue::Finished),
                cx,
            );
        }
        self.schedule_mute_expiry(now, cx);
        let prefs = &self.workspace.preferences;
        self.notification_state.approvals.update(cx, |notices, cx| {
            notices.sync(
                &snapshots,
                |session, at| {
                    let occurred = at
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                        .min(u64::MAX as u128) as u64;
                    directories.get(&session.session_id).is_some_and(|cwd| {
                        prefs.allows_project_notification(cwd, "agentInput", Some(occurred), now)
                    })
                },
                cx,
            )
        });
        let due = self
            .reminders(cx)
            .iter()
            .filter(|r| r.due_at <= now)
            .filter_map(|r| {
                let summary = self.session_summaries.get(&r.session_id)?;
                prefs
                    .allows_project_notification(&summary.cwd, "reminders", Some(r.due_at), now)
                    .then(|| reminders::Snapshot {
                        session_id: r.session_id,
                        due_at: r.due_at,
                        title: summary.title.clone(),
                        project: project_name(&summary.cwd),
                        time_label: local_reminder_label(r.due_at),
                    })
            })
            .collect();
        let error = self.reminder_error(cx).map(str::to_owned);
        self.notification_state.reminders.update(cx, |notices, cx| {
            notices.sync(due, error, prefs.notifications, cx)
        });
    }

    fn schedule_mute_expiry(&mut self, now: u64, cx: &mut Context<Self>) {
        let expiry =
            delivery::next_mute_expiry(&self.workspace.preferences.project_notifications, now);
        if self
            .notification_state
            .mute_expiry
            .as_ref()
            .map(|(at, _)| *at)
            == expiry
        {
            return;
        }
        self.notification_state.mute_expiry = expiry.map(|at| {
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(at - now))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.notification_state.mute_expiry = None;
                    cx.notify();
                });
            });
            (at, task)
        });
    }

    pub(super) fn deliver_session_notice(
        &mut self,
        summary: &Session,
        category: &str,
        body: &str,
        cue: Option<crate::sound::Cue>,
        cx: &mut Context<Self>,
    ) {
        self.dispatch_notice(
            summary,
            category,
            Target::Session(summary.id),
            body,
            cue,
            cx,
        );
    }

    pub(super) fn deliver_reminder_notice(
        &mut self,
        summary: &Session,
        due_at: u64,
        cx: &mut Context<Self>,
    ) {
        self.dispatch_notice(
            summary,
            "reminders",
            Target::Reminder {
                session_id: summary.id,
                due_at,
            },
            &summary.title,
            None,
            cx,
        );
    }

    fn dispatch_notice(
        &mut self,
        summary: &Session,
        category: &str,
        target: Target,
        body: &str,
        cue: Option<crate::sound::Cue>,
        cx: &mut Context<Self>,
    ) {
        let now = vorkspace_core::time::now_ms();
        let occurred = match target {
            Target::Session(_) => now,
            Target::Reminder { due_at, .. } => due_at,
        };
        let prefs = &self.workspace.preferences;
        if !prefs.allows_project_notification(&summary.cwd, category, Some(occurred), now) {
            return;
        }
        let notify = delivery::should_notify(
            prefs.notifications,
            self.app_core
                .as_ref()
                .map_or(Permission::Unsupported, |core| {
                    core.read(cx).notification_permission
                }),
            self.window_active,
            category != "reminders" && self.notice_session_visible(summary.id, cx),
        );
        if !notify {
            if prefs.sounds
                && occurred >= self.notification_state.sounds_enabled_at
                && let Some(cue) = cue
            {
                crate::sound::play(cue, self.store.clone());
            }
            return;
        }
        self.notification_state.sequence = self.notification_state.sequence.wrapping_add(1);
        let body = if body.trim().is_empty() && category == "agentFinished" {
            format!("{} finished", Kind::display_label(&summary.provider))
        } else {
            body.to_owned()
        };
        let receiver = platform::show(platform::Notification {
            tag: target.tag(self.notification_state.sequence),
            subtitle: if category == "reminders" {
                "Session reminder".into()
            } else {
                summary.title.clone()
            },
            body: delivery::clip_body(&body),
            sound: prefs.sounds,
        });
        let directory = summary.cwd.clone();
        let category = category.to_owned();
        let work = cx.background_executor().spawn(async move {
            receiver
                .recv_timeout(Duration::from_secs(5))
                .ok()
                .is_some_and(|result| result.is_ok())
        });
        cx.spawn(async move |this, cx| {
            let accepted = work.await;
            let _ = this.update(cx, |this, _| {
                let now = vorkspace_core::time::now_ms();
                let prefs = &this.workspace.preferences;
                if !accepted
                    && prefs.sounds
                    && occurred >= this.notification_state.sounds_enabled_at
                    && prefs.allows_project_notification(&directory, &category, Some(occurred), now)
                    && let Some(cue) = cue
                {
                    crate::sound::play(cue, this.store.clone());
                }
            });
        })
        .detach();
    }

    pub(super) fn open_notice_target(&mut self, target: Target, cx: &mut Context<Self>) {
        let core = self.app_core.clone();
        cx.spawn(async move |this, cx| {
            let id = match target {
                Target::Session(id) | Target::Reminder { session_id: id, .. } => id,
            };
            let owner = cx
                .update(|cx| {
                    core.as_ref()?
                        .read(cx)
                        .session_owner(id, cx)
                        .map(|owner| owner.downgrade())
                })
                .filter(|owner| owner != &this);
            let show = |shell: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                shell.show_notice_target(target, window, cx)
            };
            if owner
                .and_then(|owner| owner.update_in(cx, show).ok())
                .is_none()
            {
                let _ = this.update_in(cx, show);
            }
        })
        .detach();
    }

    fn show_notice_target(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let (id, due) = match target {
            Target::Session(id) => (id, None),
            Target::Reminder { session_id, due_at } => (session_id, Some(due_at)),
        };
        self.close_settings(window, cx);
        self.leave_app_views(cx);
        self.leave_app_search(cx);
        if let Some(bots) = self.bots.clone()
            && bots.update(cx, |m, cx| m.open_notification_session(id, window, cx))
        {
            if let Some(due) = due {
                self.clear_notification_reminder(id, due, cx);
            }
            window.activate_window();
            return true;
        }
        // Habit runs share their live Conversation with the Bot
        // manager. Adopt that entity so controls answer the running
        // provider, rather than constructing a disconnected history.
        if !self.sessions.contains_key(&id)
            && let Some(view) = self.notification_chat(id, cx)
        {
            let summary = view.read(cx).summary.clone();
            self.session_summaries.insert(id, summary.clone());
            self.session_marks
                .insert(id, cx.new(|_| SessionMark::new(summary.provider.clone())));
            self.tab_marks
                .insert(id, cx.new(|_| SessionMark::tab(summary.provider.clone())));
            self.observations.push(cx.subscribe(
                &view,
                move |this, _, change: &SessionChanged, cx| {
                    this.refresh_session_mark(id, cx);
                    if change.affects_attention() {
                        this.notification_session_changed(id, cx);
                    }
                    this.invalidate_chrome(cx);
                    cx.notify();
                },
            ));
            self.sessions.insert(id, view);
            if !self.order.contains(&id) {
                self.order.push(id);
            }
        }
        if !self.session_summaries.contains_key(&id) {
            return false;
        }
        let revision = self.navigation_revision;
        self.open_with_reminder(id, due, window, cx);
        if self.navigation_revision == revision || self.workspace.active != Some(id) {
            return false;
        }
        window.activate_window();
        true
    }

    pub(super) fn clear_notification_reminder(
        &mut self,
        id: i64,
        expected: u64,
        cx: &mut Context<Self>,
    ) {
        self.edit_reminder(
            app_core::ReminderEdit::Clear {
                session: id,
                due_at: expected,
            },
            cx,
        );
    }

    fn snooze_notification_reminder(
        &mut self,
        id: i64,
        expected: u64,
        due_at: u64,
        cx: &mut Context<Self>,
    ) {
        self.edit_reminder(
            app_core::ReminderEdit::Snooze {
                session: id,
                due_at: expected,
                until: due_at,
            },
            cx,
        );
    }

    fn reload_notification_reminders(&self, cx: &mut Context<Self>) {
        if let Some(core) = &self.app_core {
            core.update(cx, |core, cx| core.retry_reminders(cx));
        }
    }

    pub(super) fn open_whats_new(&mut self, version: String, cx: &mut Context<Self>) {
        let view = cx.new(|cx| WhatsNew::new(version, cx));
        let events = cx.subscribe(&view, |this, _, _: &WhatsNewEvent, cx| {
            this.notification_state.whats_new = None;
            cx.notify();
        });
        self.notification_state.whats_new = Some((view, events, cx.focus_handle()));
        cx.notify();
    }

    pub(super) fn notices_overlay(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.notification_state;
        let bot_error = self.bots.as_ref().and_then(|bots| {
            bots.read(cx)
                .error
                .clone()
                .map(|message| (bots.clone(), message))
        });
        let stack = div()
            .id("notification-stack")
            .absolute()
            .top(px(12.))
            .right(px(12.))
            .w(px(360.))
            .max_w(px((self.viewport_width - 24.).max(0.)))
            .flex()
            .flex_col()
            .items_end()
            .gap(px(8.))
            .child(state.region.probe())
            .when_some(self.error.clone(), |stack, message| {
                stack.child(errors::card(
                    "app-error",
                    message,
                    icon_button("dismiss-error", "Dismiss error", "close")
                        .debug_selector(|| "dismiss-error".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.error = None;
                            cx.notify();
                        })),
                ))
            })
            .when_some(bot_error, |stack, (bots, message)| {
                stack.child(errors::card(
                    "bot-error",
                    message,
                    icon_button("bot-error-close", "Dismiss Bot error", "close")
                        .debug_selector(|| "bot-error-close".into())
                        .on_click(cx.listener(move |_, _, _, cx| {
                            bots.update(cx, |bots, cx| bots.dismiss_error(cx));
                        })),
                ))
            })
            .when(state.file_error.read(cx).is_visible(), |stack| {
                stack.child(state.file_error.clone())
            })
            .when(!state.reminders.read(cx).is_empty(), |d| {
                d.child(
                    div()
                        .w(px(320.))
                        .max_w_full()
                        .child(state.reminders.clone()),
                )
            })
            .when(state.harness.read(cx).is_visible(), |d| {
                d.child(div().w(px(340.)).max_w_full().child(state.harness.clone()))
            })
            .when(!state.approvals.read(cx).is_empty(), |d| {
                d.child(
                    div()
                        .w(px(360.))
                        .max_w_full()
                        .child(state.approvals.clone()),
                )
            });
        // Toasts remain actionable above dialogs, including Bot settings.
        gpui::deferred(stack).with_priority(110).into_any_element()
    }

    pub(super) fn notice_modals(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut elements = Vec::new();
        if let Some((view, _, focus)) = &self.notification_state.whats_new {
            let view = view.clone();
            let focus = focus.clone();
            if !focus.contains_focused(window, cx) {
                window.focus(&focus, cx);
            }
            elements.push(
                div()
                    .id("whats-new-overlay")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui::rgba(0x00000066))
                    .track_focus(&focus)
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            this.notification_state.whats_new = None;
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.notification_state.whats_new = None;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(view),
                    )
                    .into_any_element(),
            );
        }
        elements
    }
}

impl Shell {
    fn bind_linked_activity(&mut self, cx: &mut Context<Self>) {
        self.observations.push(cx.subscribe(
            &self.notification_state.github,
            |this, _, event, cx| {
                if matches!(event, GithubActivityEvent::Changed { .. }) {
                    this.sync_linked_activity(cx);
                    cx.notify();
                }
            },
        ));
        let metadata = self.workspace.desktop_preferences_metadata.clone();
        if let Some(core) = &self.app_core {
            core.update(cx, |core, cx| core.start_linked_activity(metadata, cx));
        }
    }

    pub(super) fn complete_linked_cleanup(
        &mut self,
        id: i64,
        succeeded: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.notification_state.linked_cleanup.remove(&id) {
            return;
        }
        if let Some(notice) = self.notification_state.linked.get(&id) {
            notice.update(cx, |notice, cx| notice.finish_cleanup(cx));
        }
        if succeeded {
            self.notification_state
                .github
                .update(cx, |feed, cx| feed.acknowledge(id, cx));
        }
    }

    fn linked_activity_allowed(&self, card: &github_activity::Card) -> bool {
        let project = format!(
            "repository:github.com/{}",
            card.item.repo.trim().to_lowercase()
        );
        let category = if card.item.kind == "pr" {
            "pullRequests"
        } else {
            "issues"
        };
        let occurred = u64::try_from(card.updated_at).ok();
        occurred.is_some()
            && self.workspace.preferences.allows_notification_project(
                &project,
                category,
                occurred,
                vorkspace_core::time::now_ms(),
            )
    }

    pub(super) fn should_chime_linked_activity(
        &self,
        card: &github_activity::Card,
        started_at: u64,
    ) -> bool {
        let occurred = u64::try_from(card.updated_at).unwrap_or(0);
        self.linked_activity_allowed(card)
            && self.workspace.preferences.sounds
            && occurred >= self.notification_state.sounds_enabled_at
            && occurred >= started_at
    }

    fn linked_card(&self, id: i64, cx: &App) -> Option<github_activity::Card> {
        let chat = self.notification_chat(id, cx);
        let current = chat
            .as_ref()
            .map(|chat| &chat.read(cx).summary)
            .or_else(|| self.session_summaries.get(&id))?;
        self.notification_state
            .github
            .read(cx)
            .cards
            .get(&id)
            .filter(|card| {
                !current.archived
                    && current.work_item.as_ref() == Some(&card.item)
                    && card.updated_at as u64 > current.updated_at
                    && self.linked_activity_allowed(card)
            })
            .cloned()
    }

    fn sync_linked_activity(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<_> = self.notification_state.linked.keys().copied().collect();
        for id in ids {
            let card = self.linked_card(id, cx);
            let can_cleanup = self.session_summaries.get(&id).is_some_and(|s| !s.archived);
            if let Some(notice) = self.notification_state.linked.get(&id) {
                notice.update(cx, |notice, cx| {
                    notice.set_snapshot(card, can_cleanup, can_cleanup, cx)
                });
            }
        }
    }

    /// The pane host places the card inside the corresponding chat surface.
    pub(super) fn linked_notice(
        &mut self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<Entity<LinkedActivity>> {
        let card = self.linked_card(id, cx)?;
        let can_cleanup = self.session_summaries.get(&id).is_some_and(|s| !s.archived);
        if let Some(notice) = self.notification_state.linked.get(&id) {
            notice.update(cx, |notice, cx| {
                notice.set_snapshot(Some(card), can_cleanup, can_cleanup, cx)
            });
            return Some(notice.clone());
        }
        let notice = cx.new(|cx| {
            let mut notice = LinkedActivity::new();
            notice.set_snapshot(Some(card), can_cleanup, can_cleanup, cx);
            notice
        });
        self.observations
            .push(cx.subscribe(&notice, |this, _, event, cx| {
                this.handle_linked_activity(event.clone(), cx)
            }));
        self.notification_state.linked.insert(id, notice.clone());
        Some(notice)
    }

    fn handle_linked_activity(&mut self, event: LinkedActivityEvent, cx: &mut Context<Self>) {
        match event {
            LinkedActivityEvent::Acknowledge(id) | LinkedActivityEvent::Dismiss(id) => {
                self.notification_state
                    .github
                    .update(cx, |feed, cx| feed.acknowledge(id, cx));
            }
            LinkedActivityEvent::OpenUrl(url) => cx.open_url(&url),
            LinkedActivityEvent::OpenDiscussion(id) => {
                // A native threaded discussion pane is not implemented yet.
                if let Some(item) = self
                    .session_summaries
                    .get(&id)
                    .and_then(|s| s.work_item.as_ref())
                {
                    cx.open_url(&item.url);
                }
            }
            LinkedActivityEvent::AddToChat(id, prompt) => {
                cx.spawn(async move |this, cx| {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.close_settings(window, cx);
                        let opened_bot = this.bots.clone().is_some_and(|bots| {
                            bots.update(cx, |m, cx| m.open_notification_session(id, window, cx))
                        });
                        if !opened_bot {
                            this.open(id, window, cx);
                        }
                        if let Some(view) = this.notification_chat(id, cx) {
                            let input = view.read(cx).input.clone();
                            if !input.read(cx).is_composing() {
                                input.update(cx, |input, cx| {
                                    let draft = input.content.to_string();
                                    let separator = if draft.is_empty() || draft.ends_with("\n\n") {
                                        ""
                                    } else if draft.ends_with('\n') {
                                        "\n"
                                    } else {
                                        "\n\n"
                                    };
                                    input.set_text(
                                        format!("{draft}{separator}{}\n\n", prompt.trim()),
                                        cx,
                                    );
                                });
                                window.focus(&input.focus_handle(cx), cx);
                            }
                        }
                        window.activate_window();
                        cx.notify();
                    });
                })
                .detach();
            }
            LinkedActivityEvent::Archive(id) => {
                self.toggle_archive(id, cx);
                if self.archiving.contains(&id) {
                    self.notification_state.linked_cleanup.insert(id);
                } else if let Some(notice) = self.notification_state.linked.get(&id) {
                    notice.update(cx, |notice, cx| notice.finish_cleanup(cx));
                }
            }
            LinkedActivityEvent::Delete(id) => {
                cx.spawn(async move |this, cx| {
                    let prompt = this.update_in(cx, |_, window, cx| {
                        window.prompt(
                            gpui::PromptLevel::Warning,
                            "Delete this conversation permanently?",
                            Some("Its messages and saved settings will be removed."),
                            &["Delete", "Cancel"],
                            cx,
                        )
                    });
                    let confirmed = match prompt {
                        Ok(prompt) => matches!(prompt.await, Ok(0)),
                        Err(_) => false,
                    };
                    let _ = this.update_in(cx, |this, window, cx| {
                        if confirmed {
                            this.delete_session(id, window, cx);
                        }
                        if this.deleting.contains(&id) {
                            this.notification_state.linked_cleanup.insert(id);
                        } else if let Some(notice) = this.notification_state.linked.get(&id) {
                            notice.update(cx, |notice, cx| notice.finish_cleanup(cx));
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod announcement_tests;
#[cfg(test)]
mod error_tests;
