//! Sidebar.tsx's session actions: pins, renames, GitHub links, reminders,
//! folders and deletion, and the grouped list they shape.
use super::*;
use chrono::{Local, NaiveDateTime, TimeZone};
use vorkspace_core::{
    session_groups::{format_reminder, parse_github_work_item, reminder_at, reminder_presets},
    store::Reminder,
};
use vorkspace_ui::radius;

actions!(
    session_groups,
    [CancelRename, RenameSidebarItem, DeleteSidebarSelection]
);

pub(super) fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CancelRename, Some("SessionRename")),
        KeyBinding::new("f2", RenameSidebarItem, Some("Workspace && !SessionRename")),
        KeyBinding::new(
            "backspace",
            DeleteSidebarSelection,
            Some("SessionSidebar && !Composer"),
        ),
        KeyBinding::new(
            "delete",
            DeleteSidebarSelection,
            Some("SessionSidebar && !Composer"),
        ),
    ]);
}

fn local_ms(at: NaiveDateTime) -> Option<u64> {
    Local
        .from_local_datetime(&at)
        .earliest()
        .map(|at| at.timestamp_millis().max(0) as u64)
}
fn local_time(ms: u64) -> Option<NaiveDateTime> {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|at| at.with_timezone(&Local).naive_local())
}
pub(super) fn local_reminder_label(ms: u64) -> String {
    local_time(ms).map(format_reminder).unwrap_or_default()
}

#[derive(Clone, PartialEq)]
pub(super) enum RenameTarget {
    Session(i64),
}
pub(super) struct Renaming {
    pub(super) target: RenameTarget,
    input: Entity<input::Input>,
    _events: Vec<Subscription>,
}
pub(super) struct LinkDialog {
    id: i64,
    input: Entity<input::Input>,
    error: Option<SharedString>,
    editing: bool,
    _events: Vec<Subscription>,
}

impl Shell {
    pub(super) fn rename_sidebar_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace.project_rail_open {
            return;
        }
        let target = if self.sidebar_focus.is_focused(window) {
            self.sidebar_target.clone()
        } else {
            self.session_hover
                .or(self.workspace.active)
                .map(RenameTarget::Session)
        };
        if let Some(RenameTarget::Session(id)) = target
            && self.sidebar_session_ids(cx).contains(&id)
        {
            self.start_rename(RenameTarget::Session(id), window, cx);
        }
    }
    pub(super) fn delete_sidebar_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<_> = self.selected_sessions.iter().copied().collect();
        self.request_delete_sessions(ids, window, cx);
    }

    /// Ask for consent to permanently delete the exact IDs supplied by the caller.
    pub(super) fn request_delete_sessions(
        &mut self,
        mut ids: Vec<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        ids.sort_unstable();
        ids.dedup();
        ids.retain(|id| self.session_summaries.contains_key(id));
        if ids.is_empty() {
            return;
        }
        let removal_epoch = self.session_removal_epoch;
        let title = if ids.len() == 1 {
            "Delete this conversation permanently?".to_owned()
        } else {
            format!("Delete {} conversations permanently?", ids.len())
        };
        let prompt = window.prompt(
            gpui::PromptLevel::Warning,
            &title,
            Some("Messages and saved settings will be removed."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if prompt.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                // Numeric IDs may be reused after removal; never apply old consent
                // to a replacement, even if the selection now looks identical.
                if this.session_removal_epoch != removal_epoch {
                    this.error = Some(
                        "The conversation list changed. Please confirm deletion again.".into(),
                    );
                    cx.notify();
                    return;
                }
                for id in ids {
                    this.delete_session(id, window, cx);
                }
            });
        })
        .detach();
    }
    pub(super) fn store_write(
        &mut self,
        what: &'static str,
        cx: &mut Context<Self>,
        write: impl FnOnce(&mut vorkspace_core::store::Store) -> Result<(), String> + Send + 'static,
    ) {
        let store = self.store.clone();
        let previous = self.group_writes.take();
        self.group_writes = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx
                .background_executor()
                .spawn(async move { write(&mut *locked(&store)?) });
            if let Err(error) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!("Could not {what}: {error}"));
                    cx.notify();
                });
            }
        }));
    }

    /// The sessions in the order the rail shows them, for stepping.
    pub(super) fn sidebar_navigation_ids(&self, cx: &App) -> Vec<i64> {
        self.rail_session_ids(cx)
    }

    fn session_menu_ids(&self, id: i64, cx: &App) -> Vec<i64> {
        if !self.selected_sessions.contains(&id) {
            return vec![id];
        }
        self.sidebar_session_ids(cx)
            .into_iter()
            .filter(|id| self.selected_sessions.contains(id))
            .collect()
    }
    pub(super) fn clear_context_selection(&mut self) {
        if std::mem::take(&mut self.context_selection) {
            self.selected_sessions.clear();
        }
    }
    pub(super) fn select_sidebar_session(
        &mut self,
        id: i64,
        modifiers: gpui::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_selection = false;
        self.close_menu(cx);
        self.sidebar_target = Some(RenameTarget::Session(id));
        let additive = modifiers.control || modifiers.platform;
        if modifiers.shift {
            let visible = self.sidebar_navigation_ids(cx);
            let anchor = self
                .selection_anchor
                .filter(|id| visible.contains(id))
                .or(self.workspace.active)
                .unwrap_or(id);
            let range = visible
                .iter()
                .position(|s| *s == anchor)
                .zip(visible.iter().position(|s| *s == id));
            if !additive {
                self.selected_sessions.clear();
            }
            if let Some((a, b)) = range {
                self.selected_sessions.extend(&visible[a.min(b)..=a.max(b)]);
                self.selection_anchor = Some(anchor);
            } else {
                self.selected_sessions.insert(id);
                self.selection_anchor = Some(id);
            }
        } else if additive {
            self.selection_anchor = Some(id);
            if !self.selected_sessions.insert(id) {
                self.selected_sessions.remove(&id);
            }
        } else {
            self.selected_sessions.clear();
            self.selection_anchor = Some(id);
            self.open(id, window, cx);
        }
        if modifiers.shift || additive {
            window.focus(&self.sidebar_focus, cx);
        }
        self.invalidate_chrome(cx);
        cx.notify();
    }
    pub(super) fn session_menu_items(&self, id: i64, cx: &App) -> Vec<MenuItem> {
        let summary = self.session_metadata(id, cx).summary;
        let ids = self.session_menu_ids(id, cx);
        let multiple = ids.len() > 1;
        let all_pinned = ids
            .iter()
            .all(|id| self.session_metadata(*id, cx).summary.pinned);
        let all_archived = ids
            .iter()
            .all(|id| self.session_metadata(*id, cx).summary.archived);
        let mut items = Vec::new();
        if self.reminders.iter().any(|r| ids.contains(&r.session_id)) {
            items.push(
                MenuAction::new("reminder:cancel", "Cancel reminder")
                    .icon("circle-x")
                    .into(),
            );
            items.push(MenuItem::Separator);
        }
        items.push(
            MenuAction::new("pin", if all_pinned { "Unpin" } else { "Pin" })
                .icon("pin")
                .into(),
        );
        if !multiple {
            items.extend([
                MenuAction::new("rename", "Rename")
                    .icon("pencil-edit")
                    .shortcut("F2")
                    .into(),
                MenuAction::new("copy-session-id", "Copy session ID")
                    .icon("response-copy")
                    .submenu(vec![
                        MenuAction::new("copy-harness-session-id", "Harness session ID")
                            .disabled(summary.provider_id.is_none()),
                        MenuAction::new("copy-vorkspace-session-id", "vorkspace session ID"),
                    ])
                    .into(),
                MenuAction::new(
                    "link-work-item",
                    if summary.work_item.is_some() {
                        "Edit GitHub issue or PR link…"
                    } else {
                        "Link GitHub issue or PR…"
                    },
                )
                .icon("external-link")
                .into(),
            ]);
        }
        items.extend([MenuAction::new("reminder", "Remind me")
            .icon("clock")
            .submenu(
                reminder_presets(Local::now().naive_local())
                    .into_iter()
                    .map(|(id, label, enabled)| MenuAction::new(id, label).disabled(!enabled))
                    .collect(),
            )
            .into()]);
        items.extend([
            MenuItem::Separator,
            MenuAction::new(
                "archive",
                if all_archived { "Unarchive" } else { "Archive" },
            )
            .icon("archive")
            .into(),
            MenuAction::new("delete", "Delete")
                .icon("trash")
                .shortcut("⌫")
                .danger()
                .into(),
        ]);
        items
    }
    pub(super) fn run_session_menu(
        &mut self,
        id: i64,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.session_summaries.contains_key(&id) {
            return;
        }
        let summary = self.session_metadata(id, cx).summary.clone();
        let ids = self.session_menu_ids(id, cx);
        match action {
            "copy-harness-session-id" | "copy-vorkspace-session-id" => {
                let text = if action == "copy-harness-session-id" {
                    summary.provider_id
                } else {
                    locked(&self.store)
                        .and_then(|store| store.persisted_session_id(summary.id))
                        .ok()
                };
                if let Some(text) = text {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            "pin" => {
                let pinned = !ids
                    .iter()
                    .all(|id| self.session_metadata(*id, cx).summary.pinned);
                for id in &ids {
                    if let Some(summary) = self.session_summaries.get_mut(id) {
                        summary.pinned = pinned;
                    }
                    if let Some(view) = self.sessions.get(id) {
                        view.update(cx, |s, _| s.summary.pinned = pinned);
                    }
                }
                self.store_write("pin the session", cx, move |s| s.set_pinned(&ids, pinned));
                self.invalidate_chrome(cx);
            }
            "rename" => self.start_rename(RenameTarget::Session(id), window, cx),
            "link-work-item" => self.open_link_dialog(id, window, cx),
            "reminder:cancel" => {
                let occurrences: Vec<_> = self
                    .reminders
                    .iter()
                    .filter(|reminder| ids.contains(&reminder.session_id))
                    .map(|reminder| (reminder.session_id, reminder.due_at))
                    .collect();
                for (id, due_at) in occurrences {
                    self.clear_notification_reminder(id, due_at, cx);
                }
                self.invalidate_chrome(cx);
            }
            preset if preset.starts_with("reminder:") => {
                let Some(due_at) =
                    reminder_at(preset, Local::now().naive_local()).and_then(local_ms)
                else {
                    return;
                };
                self.reminders.retain(|r| !ids.contains(&r.session_id));
                self.reminders.extend(ids.iter().map(|id| Reminder {
                    session_id: *id,
                    due_at,
                    fired_at: None,
                }));
                self.invalidate_chrome(cx);
                self.store_write("set the reminder", cx, move |s| {
                    s.set_reminders(&ids, due_at)
                });
            }
            "archive" => {
                let archived = !ids
                    .iter()
                    .all(|id| self.session_metadata(*id, cx).summary.archived);
                for id in ids {
                    if self.session_metadata(id, cx).summary.archived != archived {
                        self.toggle_archive(id, cx);
                    }
                }
            }
            "delete" => self.request_delete_sessions(ids, window, cx),
            _ => {}
        }
    }

    /// Opening a session whose reminder is due dismisses it.
    pub(super) fn dismiss_due_reminder(&mut self, id: i64, cx: &mut Context<Self>) {
        let now = vorkspace_core::time::now_ms();
        let Some(due) = self
            .reminders
            .iter()
            .find(|r| r.session_id == id && r.due_at <= now)
            .map(|r| r.due_at)
        else {
            return;
        };
        self.clear_notification_reminder(id, due, cx);
    }

    pub(super) fn poll_reminders(&mut self, cx: &mut Context<Self>) {
        if self.notification_state.reminder_error.is_some() {
            return;
        }
        let now = vorkspace_core::time::now_ms();
        let due: Vec<_> = self
            .reminders
            .iter_mut()
            .filter_map(|reminder| {
                if reminder.due_at > now || reminder.fired_at.is_some() {
                    return None;
                }
                reminder.fired_at = Some(now);
                Some((reminder.session_id, reminder.due_at))
            })
            .collect();
        if due.is_empty() {
            return;
        }
        let store = self.store.clone();
        let previous = self.group_writes.take();
        self.group_writes = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx
                .background_executor()
                .spawn(async move { locked(&store)?.take_due_reminders(now) });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(claimed) => {
                        this.notification_state.reminder_error = None;
                        for reminder in claimed {
                            // A cancel/reschedule while the store operation was
                            // pending must never deliver an obsolete occurrence.
                            if this.reminders.iter().any(|current| {
                                current.session_id == reminder.session_id
                                    && current.due_at == reminder.due_at
                            }) {
                                this.deliver_reminder_notice(
                                    reminder.session_id,
                                    reminder.due_at,
                                    cx,
                                );
                            }
                        }
                    }
                    Err(error) => {
                        // Re-arm only the occurrences claimed by this request;
                        // a newer schedule has its own independent fired state.
                        for reminder in &mut this.reminders {
                            if due.contains(&(reminder.session_id, reminder.due_at))
                                && reminder.fired_at == Some(now)
                            {
                                reminder.fired_at = None;
                            }
                        }
                        this.notification_state.reminder_error =
                            Some(format!("Could not deliver reminders: {error}"));
                    }
                }
                this.invalidate_chrome(cx);
                cx.notify();
            });
        }));
        self.invalidate_chrome(cx);
    }

    fn report_session_delete_error(&mut self, id: i64, reason: &str) {
        let message = format!("Could not delete conversation {id}: {reason}");
        match &mut self.error {
            Some(error) if error.starts_with("Could not delete conversation ") => {
                error.push('\n');
                error.push_str(&message);
            }
            _ => self.error = Some(message),
        }
    }

    pub(super) fn delete_session(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.review_owners.iter().any(|(pane, owner)| {
            *owner == id
                && (self.workspace.tab_of(*pane).is_some()
                    || self
                        .reviews
                        .get(pane)
                        .is_some_and(|r| r.read(cx).unsaved_files(cx)))
        }) {
            self.report_session_delete_error(id, "close its detached file panes first");
            cx.notify();
            return;
        }
        if !self.session_summaries.contains_key(&id) || !self.deleting.insert(id) {
            return;
        }
        self.close_preview(cx);
        if let Some(view) = self.sessions.get(&id) {
            view.update(cx, |session, cx| session.stop_if_busy(cx));
        }
        if self.workspace.tab_of(id).is_some() {
            self.close_pane(id, window, cx);
        }
        if self.workspace.tab_of(id).is_some() {
            self.deleting.remove(&id);
            self.report_session_delete_error(id, "it is still open");
            cx.notify();
            return;
        }
        self.invalidate_chrome(cx);
        let store = self.store.clone();
        let previous = self.group_writes.take();
        self.group_writes = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx.background_executor().spawn(async move {
                locked(&store)?.delete(id)?;
                // Drop the session's recorded turns and their Git refs.
                if let Some(checkpoints) = vorkspace_core::checkpoint::store() {
                    let _ = checkpoints.exclusive(|c| c.forget(&id.to_string()));
                }
                Ok::<_, String>(())
            });
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.deleting.remove(&id);
                match result {
                    Ok(()) => this.forget_session(id, cx),
                    Err(error) => this.report_session_delete_error(id, &error),
                }
                this.invalidate_chrome(cx);
                this.sync_settings(cx);
                cx.notify();
            });
        }));
    }
    pub(super) fn forget_session(&mut self, id: i64, cx: &mut Context<Self>) {
        // A tab opened on it while the store was deleting keeps it alive.
        if self.workspace.tab_of(id).is_some() {
            return;
        }
        if self
            .session_preview
            .as_ref()
            .is_some_and(|preview| preview.id == id)
        {
            self.session_preview = None;
            self.preview_close = None;
        }
        self.sessions.remove(&id);
        self.session_removal_epoch = self.session_removal_epoch.wrapping_add(1);
        self.session_summaries.remove(&id);
        self.draft_sessions.remove(&id);
        self.history_recency.remove(&id);
        self.session_marks.remove(&id);
        self.tab_marks.remove(&id);
        self.order.retain(|s| *s != id);
        self.forget_search_session(id, cx);
        self.busy_sessions.remove(&id);
        self.unseen_finished.remove(&id);
        self.selected_sessions.remove(&id);
        self.reminders.retain(|r| r.session_id != id);
        self.workspace.project_focus.retain(|_, s| *s != id);
        self.history
            .prune(&self.workspace.open_sessions(), self.workspace.active);
        if self.session_hover == Some(id) {
            self.session_hover = None;
        }
        if self
            .renaming
            .as_ref()
            .is_some_and(|r| r.target == RenameTarget::Session(id))
        {
            self.renaming = None;
        }
        self.persist(cx);
    }

    pub(super) fn start_rename(
        &mut self,
        target: RenameTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_preview(cx);
        let text = match &target {
            RenameTarget::Session(id) => self.session_metadata(*id, cx).display_title(),
        };
        let input = cx.new(|cx| {
            let mut input = input::Input::field(
                "Rename. Press Enter to confirm or Escape to cancel.",
                "",
                content(0.35),
                cx,
            );
            input.set_font_size(14.);
            input
        });
        let len = text.len();
        input.update(cx, |i, cx| {
            i.set_text(text, cx);
            i.select_range(0..len, cx);
        });
        let focus = input.read(cx).focus_handle(cx);
        let events = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, _, _: &input::Submitted, window, cx| this.finish_rename(true, window, cx),
            ),
            cx.on_blur(&focus, window, |this, window, cx| {
                this.finish_rename(true, window, cx)
            }),
        ];
        window.focus(&focus, cx);
        self.renaming = Some(Renaming {
            target,
            input,
            _events: events,
        });
        self.invalidate_chrome(cx);
        cx.notify();
    }
    fn finish_rename(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let name = renaming.input.read(cx).content.trim().to_string();
        if commit && !name.is_empty() {
            match renaming.target {
                RenameTarget::Session(id) => {
                    if self.session_summaries.contains_key(&id)
                        && self.session_metadata(id, cx).summary.title != name
                    {
                        if let Some(summary) = self.session_summaries.get_mut(&id) {
                            summary.title = name.clone();
                        }
                        if let Some(session) = self.sessions.get(&id) {
                            session.update(cx, |s, cx| {
                                s.rename(name.clone(), cx);
                            });
                        } else {
                            self.sync_search_session(id, cx);
                        }
                        self.store_write("rename the session", cx, move |s| s.title(id, &name));
                    }
                }
            }
        }
        if renaming.input.read(cx).focus_handle(cx).is_focused(window) {
            self.focus_active(window, cx);
        }
        self.invalidate_chrome(cx);
        cx.notify();
    }
    /// SessionRenameRow: the name in a focused field.
    pub(super) fn rename_row(
        &self,
        target: &RenameTarget,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let renaming = self.renaming.as_ref().filter(|r| r.target == *target)?;
        Some(
            div()
                .key_context("SessionRename")
                .on_action(cx.listener(|this, _: &CancelRename, window, cx| {
                    this.finish_rename(false, window, cx)
                }))
                .h(px(32.))
                .px_2()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(22.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(radius::SMALL))
                        .bg(content(0.1))
                        .border_1()
                        .border_color(rgba(0x4da3f566))
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(1.))
                        .child(renaming.input.clone()),
                )
                .into_any(),
        )
    }

    /// SessionCard's linked issue or PR; Cmd/Ctrl-click and click both open it.
    pub(super) fn work_item_badge(
        &self,
        id: i64,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let item = self.session_metadata(id, cx).summary.work_item.clone()?;
        let noun = if item.kind == "pr" { "PR" } else { "issue" };
        let url = item.url.clone();
        Some(
            div()
                .id(("work-item", id as usize))
                .role(Role::Link)
                .aria_label(format!("Open {noun} #{}", item.number))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(2.))
                .px(px(2.))
                .rounded(px(radius::SMALL))
                .text_size(px(11.))
                .text_color(rgb(ACCENT))
                .child(
                    icon(
                        if item.kind == "pr" {
                            "git-pull-request"
                        } else {
                            "circle-dot"
                        },
                        12.,
                    )
                    .text_color(rgb(ACCENT)),
                )
                .when(!compact, |d| d.child(format!("#{}", item.number)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.stop_propagation();
                    cx.open_url(&url);
                }))
                .into_any(),
        )
    }

    fn open_link_dialog(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.close_preview(cx);
        let initial = self.session_metadata(id, cx).summary.work_item.clone();
        let input = cx.new(|cx| {
            input::Input::field(
                "GitHub issue or pull request URL",
                "https://github.com/owner/repo/pull/123",
                content(0.3),
                cx,
            )
        });
        if let Some(item) = &initial {
            let url = item.url.clone();
            input.update(cx, |i, cx| i.set_text(url, cx));
        }
        let events = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, _, _: &input::Submitted, window, cx| this.save_link(true, window, cx),
            ),
            cx.subscribe(&input, |this, _, _: &input::Changed, cx| {
                if let Some(dialog) = &mut this.link_dialog
                    && dialog.error.take().is_some()
                {
                    cx.notify();
                }
            }),
        ];
        window.focus(&input.read(cx).focus_handle(cx), cx);
        self.link_dialog = Some(LinkDialog {
            id,
            input,
            error: None,
            editing: initial.is_some(),
            _events: events,
        });
        cx.notify();
    }
    pub(super) fn close_link_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.link_dialog.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }
    /// Saves the parsed link, or with `keep` false, removes it.
    fn save_link(&mut self, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.link_dialog else {
            return;
        };
        let id = dialog.id;
        let item = if keep {
            match parse_github_work_item(dialog.input.read(cx).content.trim()) {
                Some(item) => Some(item),
                None => {
                    dialog.error = Some("Enter a valid GitHub issue or pull request URL.".into());
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        if let Some(summary) = self.session_summaries.get_mut(&id) {
            summary.work_item = item.clone();
        }
        if let Some(session) = self.sessions.get(&id) {
            session.update(cx, |s, _| s.summary.work_item = item.clone());
        }
        self.store_write("link the session", cx, move |s| {
            s.set_work_item(id, item.as_ref())
        });
        self.invalidate_chrome(cx);
        self.close_link_dialog(window, cx);
    }
    /// LinkSessionWorkItemDialog, centred over the window.
    pub(super) fn link_modal(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.link_dialog.as_ref()?;
        let title = self.session_metadata(dialog.id, cx).display_title();
        let viewport = window.viewport_size();
        let width = 384f32.min(viewport.width.as_f32() - 24.);
        let anchor = Anchor::Point(gpui::point(
            px((viewport.width.as_f32() - width) / 2.),
            px((viewport.height.as_f32() / 2. - 120.).max(16.)),
        ));
        let weak = cx.weak_entity();
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .map(vorkspace_ui::theme::tab_reachable)
                .cursor(CursorStyle::Arrow)
                .px_3()
                .py_1p5()
                .rounded(px(radius::CONTROL))
                .text_size(px(12.))
                .child(label)
        };
        let field = div()
            .h(px(36.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(radius::CONTROL))
            .border_1()
            .border_color(if dialog.error.is_some() {
                rgba(0xf8717199).into()
            } else {
                content(0.1)
            })
            .bg(content(0.05))
            .text_size(px(13.))
            .text_color(content(1.))
            .child(dialog.input.clone());
        Some(
            div()
                .absolute()
                .size_full()
                .bg(vorkspace_ui::fluid::scrim(0.4))
                .child(
                    popover("link-work-item-modal", anchor)
                        .modal_surface(self.workspace.classic_layout)
                        .width(width)
                        .gap(0.)
                        .priority(60)
                        .on_dismiss(move |window, cx| {
                            let _ = weak.update(cx, |this, cx| this.close_link_dialog(window, cx));
                        })
                        .child(
                            div()
                                .id("link-work-item-dialog")
                                .role(Role::Dialog)
                                .aria_label(if dialog.editing {
                                    "Edit GitHub link"
                                } else {
                                    "Link GitHub issue or PR"
                                })
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .px_4()
                                        .pt_4()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_size(px(14.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(content(1.))
                                                .child(if dialog.editing {
                                                    "Edit GitHub link"
                                                } else {
                                                    "Link GitHub issue or PR"
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(content(0.5))
                                                .truncate()
                                                .child(title),
                                        ),
                                )
                                .child(
                                    div()
                                        .p_4()
                                        .flex()
                                        .flex_col()
                                        .gap_4()
                                        .text_size(px(12.))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_1p5()
                                                .child(
                                                    div()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(content(0.8))
                                                        .child("Issue or pull request URL"),
                                                )
                                                .child(field)
                                                .child(match &dialog.error {
                                                    Some(error) => div()
                                                            .id("link-error")
                                                        .role(Role::Alert)
                                                        .aria_label(error.clone())
                                                        .text_size(px(11.))
                                                        .text_color(vorkspace_ui::theme::danger())
                                                        .child(error.clone()),
                                                    None => div()
                                                            .id("link-help")
                                                        .text_size(px(11.))
                                                        .text_color(content(0.45))
                                                        .child(
                                                            "Paste the full github.com URL. The linked item will appear on the session card.",
                                                        ),
                                                }),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_end()
                                                .gap_2()
                                                .when(dialog.editing, |d| {
                                                    d.child(
                                                        button("link-remove", "Remove link")
                                                            .mr_auto()
                                                            .text_color(vorkspace_ui::theme::danger())
                                                            .hover(|s| s.bg(vorkspace_ui::fluid::wash(rgba(0xf871711a).into())))
                                                            .on_click(cx.listener(|this, _, window, cx| {
                                                                this.save_link(false, window, cx)
                                                            })),
                                                    )
                                                })
                                                .child(
                                                    button("link-cancel", "Cancel")
                                                        .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
                                                        .on_click(cx.listener(|this, _, window, cx| {
                                                            this.close_link_dialog(window, cx)
                                                        })),
                                                )
                                                .child(
                                                    button(
                                                        "link-save",
                                                        if dialog.editing { "Update link" } else { "Link" },
                                                    )
                                                    .bg(rgb(ACCENT))
                                                    .text_color(gpui::white())
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .hover(|s| s.opacity(0.9))
                                                    .on_click(cx.listener(|this, _, window, cx| {
                                                        this.save_link(true, window, cx)
                                                    })),
                                                ),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests;
