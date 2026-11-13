//! The session filters menu: persisted status, time and provider checkboxes.
use super::*;
use vorkspace_core::session_filters::{BotFilter, TimeFilter};

impl Shell {
    pub(super) fn effective_session_filters(
        &self,
        cx: &App,
    ) -> vorkspace_core::session_filters::Filters {
        let mut filters = self.workspace.session_filters.clone();
        let projects = self.workspace.vorkspace_projects();
        if projects.len() <= 1 {
            filters.hidden_projects.clear();
        } else {
            let keys: std::collections::HashSet<String> = projects
                .iter()
                .map(|p| vorkspace_core::files::path_key(p))
                .collect();
            filters.hidden_projects.retain(|k| keys.contains(k));
        }
        if let BotFilter::Bot(ref bot_id) = filters.bot {
            let visible = self.bots.as_ref().is_some_and(|manager| {
                let m = manager.read(cx);
                !m.vorkspace_hidden.contains(bot_id)
                    && m.roster.bots.iter().any(|b| &b.id == bot_id)
            });
            if !visible {
                filters.bot = BotFilter::Any;
            }
        }
        filters
    }

    pub(super) fn session_filters_active(&self, cx: &App) -> bool {
        self.effective_session_filters(cx)
            .active(self.workspace.preferences.show_archived)
    }

    pub(super) fn filter_menu_items(&self, cx: &App) -> Vec<MenuItem> {
        let filters = self.effective_session_filters(cx);
        let mut items = vec![
            MenuAction::new("archived", "Archived")
                .checked(self.workspace.preferences.show_archived)
                .keep_open()
                .into(),
            MenuItem::Header("Status".into()),
        ];
        for (id, label, checked) in [
            ("working", "Working", filters.status.working),
            ("approval", "Needs approval", filters.status.needs_approval),
            ("done", "Done", filters.status.done),
        ] {
            items.push(
                MenuAction::new(id, label)
                    .checked(checked)
                    .keep_open()
                    .into(),
            );
        }
        items.push(MenuItem::Header("Time".into()));
        for (time, id, label) in TimeFilter::OPTIONS {
            items.push(
                MenuAction::new(format!("time:{id}"), label)
                    .checked(filters.time == time)
                    .keep_open()
                    .into(),
            );
        }
        let projects: Vec<String> = self.workspace.vorkspace_projects();
        let keys: std::collections::HashSet<String> = projects
            .iter()
            .map(|p| vorkspace_core::files::path_key(p))
            .collect();
        let providers: Vec<_> = Kind::ALL
            .into_iter()
            .filter(|kind| {
                self.order.iter().any(|id| {
                    let session = self.session_metadata(*id, cx);
                    keys.contains(&vorkspace_core::files::path_key(&session.summary.cwd))
                        && session.summary.has_messages
                        && session.summary.provider == kind.id()
                })
            })
            .collect();
        if !providers.is_empty() {
            items.push(MenuItem::Header("Provider".into()));
            for kind in providers {
                items.push(
                    MenuAction::new(format!("provider:{}", kind.id()), kind.label())
                        .image(kind.id())
                        .checked(!filters.hidden_providers.iter().any(|id| id == kind.id()))
                        .keep_open()
                        .into(),
                );
            }
        }
        if projects.len() > 1 {
            items.push(MenuItem::Header("Project".into()));
            for path in &projects {
                let key = vorkspace_core::files::path_key(path);
                items.push(
                    MenuAction::new(format!("project:{key}"), self.project_label(path))
                        .icon("folder")
                        .checked(!filters.hidden_projects.contains(&key))
                        .keep_open()
                        .into(),
                );
            }
        }
        items.push(MenuItem::Header("Bot".into()));
        // Single choice: any session, one Bot's, or ones no Bot started.
        let mut bots = vec![(BotFilter::Any, "bot:any".to_string(), "Any".to_string())];
        if let Some(manager) = &self.bots {
            let hidden = &manager.read(cx).vorkspace_hidden;
            bots.extend(
                manager
                    .read(cx)
                    .roster
                    .bots
                    .iter()
                    .filter(|bot| !hidden.contains(&bot.id))
                    .map(|bot| {
                        (
                            BotFilter::Bot(bot.id.clone()),
                            format!("bot:id:{}", bot.id),
                            bot.label(),
                        )
                    }),
            );
        }
        bots.push((BotFilter::NoBot, "bot:none".into(), "No Bot".into()));
        for (value, id, label) in bots {
            items.push(
                MenuAction::new(id, label)
                    .checked(filters.bot == value)
                    .keep_open()
                    .into(),
            );
        }
        if self.session_filters_active(cx) {
            items.push(MenuItem::Separator);
            items.push(MenuAction::new("clear", "Clear filters").keep_open().into());
        }
        items
    }

    pub(super) fn change_session_filter(&mut self, action: &str, cx: &mut Context<Self>) {
        let filters = &mut self.workspace.session_filters;
        match action {
            "archived" => {
                self.workspace.preferences.show_archived = !self.workspace.preferences.show_archived
            }
            "working" => filters.status.working = !filters.status.working,
            "approval" => filters.status.needs_approval = !filters.status.needs_approval,
            "done" => filters.status.done = !filters.status.done,
            "clear" => {
                *filters = Default::default();
                self.workspace.preferences.show_archived = false;
            }
            _ => {
                if let Some(provider) = action.strip_prefix("provider:") {
                    filters.toggle_provider(provider);
                } else if let Some(key) = action.strip_prefix("project:") {
                    filters.toggle_project(key);
                } else if let Some(bot) = action.strip_prefix("bot:") {
                    filters.bot = match bot {
                        "some" => BotFilter::AnyBot,
                        "none" => BotFilter::NoBot,
                        _ => match bot.strip_prefix("id:") {
                            Some(id) => BotFilter::Bot(id.into()),
                            None => BotFilter::Any,
                        },
                    };
                } else if let Some(time) = action.strip_prefix("time:").and_then(|id| {
                    TimeFilter::OPTIONS
                        .iter()
                        .find(|(_, value, _)| *value == id)
                        .map(|(t, _, _)| *t)
                }) {
                    filters.time = time;
                } else {
                    return;
                }
            }
        }
        self.close_preview(cx);
        let visible = self.sidebar_session_ids(cx);
        self.sidebar
            .selected_sessions
            .retain(|id| visible.contains(id));
        self.sidebar.selection_anchor = self
            .sidebar
            .selection_anchor
            .filter(|id| visible.contains(id));
        self.invalidate_chrome(cx);
        self.persist(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn filters_persist_and_update_without_closing_the_menu_or_active_chat(cx: &mut TestAppContext) {
        cx.update(|cx| {
            super::super::bind_keys(cx);
            crate::popover::bind(cx);
        });
        let (window, a, b) = super::super::tests::window(cx);
        window
            .update(cx, |shell, w, cx| {
                let project = shell.workspace.project.clone();
                for id in [a, b] {
                    shell.sessions[&id].update(cx, |s, _| {
                        s.summary.cwd = project.clone();
                        s.summary.has_messages = true;
                        s.summary.updated_at = vorkspace_core::time::now_ms();
                    });
                }
                shell.sessions[&a].update(cx, |s, _| {
                    s.summary.provider = "codex".into();
                    s.set_running_for_test(true);
                });
                shell.sessions[&b].update(cx, |s, _| s.summary.provider = "claude".into());
                shell.sidebar.selected_sessions.insert(b);
                shell.open_menu(
                    MenuTarget::SessionFilter,
                    Anchor::Point(gpui::point(px(220.), px(180.))),
                    w,
                    cx,
                );
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = visual.debug_bounds("menu-action-working").unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert!(shell.menu.is_some(), "status checkboxes keep the menu open");
                assert_eq!(shell.sidebar_session_ids(cx), [a]);
                assert_eq!(shell.workspace.active, Some(b));
                assert!(shell.sessions[&a].read(cx).responding());
                assert!(!shell.sidebar.selected_sessions.contains(&b));
                let state: Workspace =
                    serde_json::from_str(&shell.latest_window_layout(cx)).unwrap();
                assert!(state.session_filters.status.working);
            })
            .unwrap();
        window
            .update(visual, |shell, _, cx| {
                shell.change_session_filter("clear", cx);
                assert_eq!(shell.sidebar_session_ids(cx).len(), 2);
                shell.sessions[&a].update(cx, |s, _| s.set_running_for_test(false));
                shell.unseen_finished.insert(a);
                shell.change_session_filter("done", cx);
                assert_eq!(shell.sidebar_session_ids(cx), [a]);
                shell.sessions[&b].update(cx, |s, _| s.set_running_for_test(true));
                shell.change_session_filter("working", cx);
                assert_eq!(shell.sidebar_session_ids(cx).len(), 2);
                shell.change_session_filter("provider:claude", cx);
                assert_eq!(shell.sidebar_session_ids(cx), [a]);
                shell.change_session_filter("time:today", cx);
                assert_eq!(shell.sidebar_session_ids(cx), [a]);
                shell.sessions[&a].update(cx, |s, _| s.summary.updated_at = 1);
                assert!(shell.sidebar_session_ids(cx).is_empty());
            })
            .unwrap();
    }
}
