//! All title-strip commands operate on the same visible, ordered snapshot.
use super::*;
use crate::workspace::TitleTab;
use vorkspace_core::files::path_key;

impl Shell {
    /// The workspace keys a tab by one of its sessions. Replacing that session
    /// changes the key, but the same slot and selected width stay on screen.
    pub(super) fn replace_title_tab_key(&mut self, from: i64, to: i64) {
        if from == to {
            return;
        }
        let previous = TitleTab::Session(from);
        let next = TitleTab::Session(to);
        self.title_tab_motion.replace_key(&previous, next.clone());
        for selected in [
            &mut self.title_tab_selection.0,
            &mut self.title_tab_selection.1,
        ] {
            if selected.as_ref() == Some(&previous) {
                *selected = Some(next.clone());
            }
        }
        let shift = self.tab_shifts.remove(&previous);
        self.tab_shifts.remove(&next);
        if let Some(shift) = shift {
            self.tab_shifts.insert(next, shift);
        }
        self.pane_ticks.remove(&from);
        self.pane_ticks.remove(&to);
        if self.tab_hover == Some(from) {
            self.tab_hover = Some(to);
        }
    }

    pub(crate) fn click_bot_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.skip_tab_click) {
            return;
        }
        self.open_title_tab(&self.bot_tab(id.into()), window, cx);
    }
    pub(super) fn tab_scope(&self) -> String {
        format!(
            "{}\0{}",
            path_key(&self.workspace.project),
            path_key(self.working_copy())
        )
    }

    pub(super) fn bot_tab(&self, id: String) -> TitleTab {
        TitleTab::Bot {
            id,
            project: self.workspace.project.clone(),
            worktree: self.working_copy().into(),
        }
    }

    pub(super) fn visible_tabs(&self, cx: &App) -> Vec<TitleTab> {
        self.workspace
            .title_order
            .iter()
            .filter(|tab| match tab {
                TitleTab::Terminal { .. } => true,
                TitleTab::Session(tab) => {
                    self.workspace.tabs.contains(tab) && {
                        let id = self.workspace.tab_focus(*tab);
                        self.session_summaries
                            .contains_key(&self.workspace.session_for_pane(id))
                            && self
                                .session_metadata(self.workspace.session_for_pane(id), cx)
                                .summary
                                .cwd
                                == self.workspace.project
                            && self.in_worktree_scope(id, cx)
                    }
                }
                TitleTab::Search | TitleTab::App(_) | TitleTab::Bot { .. } => false,
            })
            .cloned()
            .collect()
    }
    pub(super) fn active_title_tab(&self, cx: &App) -> Option<TitleTab> {
        if let Some(active) = self.active_terminal_tab {
            return self
                .workspace
                .title_order
                .iter()
                .find(|tab| matches!(tab, TitleTab::Terminal { id, .. } if *id == active))
                .cloned();
        }
        if let Some(view) = self.active_app_view {
            return Some(TitleTab::App(view));
        }
        if self.app_search.is_some() {
            return Some(TitleTab::Search);
        }
        self.bots
            .as_ref()
            .and_then(|m| m.read(cx).active.clone())
            .map(|id| self.bot_tab(id))
            .or_else(|| self.workspace.active_tab().map(TitleTab::Session))
    }

    /// The active tab follows live focus; other tabs keep their last displayed
    /// pane instead of borrowing the currently selected tab's preview focus.
    pub(super) fn tab_review_focused(&self, tab: i64, cx: &App) -> bool {
        if self.active_title_tab(cx) == Some(TitleTab::Session(tab)) {
            self.review_focused
        } else {
            self.pane_ticks
                .get(&tab)
                .is_some_and(|tick| tick.shown == (self.workspace.tab_focus(tab), true))
        }
    }

    /// Sidebar pages fill the pane without a tab strip.
    pub(super) fn tabless_page(&self, cx: &App) -> bool {
        self.active_app_view.is_some() || self.app_search.is_some() || self.bot_active(cx)
    }

    pub(super) fn remember_title_tab(&mut self, tab: TitleTab) {
        if !tab.in_strip() {
            return;
        }
        if !self.workspace.title_order.contains(&tab) {
            self.workspace.title_order.push(tab.clone());
        }
        self.workspace.scope_focus.insert(self.tab_scope(), tab);
    }

    pub(super) fn bot_opened(&mut self, id: String, cx: &mut Context<Self>) {
        if !self
            .bots
            .as_ref()
            .is_some_and(|m| m.read(cx).active.as_deref() == Some(&id))
        {
            return;
        }
        self.navigation_revision += 1;
        self.selected_sessions.clear();
        self.selection_anchor = None;
        self.sidebar_target = None;
        self.context_selection = false;
        self.leave_app_views(cx);
        self.leave_app_search(cx);
        self.close_preview(cx);
        self.review_focused = false;
        self.remember_title_tab(self.bot_tab(id));
        self.reveal_active_tab(cx);
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn open_title_tab(
        &mut self,
        tab: &TitleTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match tab {
            TitleTab::Terminal { id, cwd } => self.show_terminal_tab(*id, cwd.clone(), window, cx),
            TitleTab::Search => self.open_app_search(window, cx),
            TitleTab::App(view) => match view {
                app_views::AppView::Inbox => self.open_inbox(window, cx),
                app_views::AppView::Connector(provider) => {
                    self.open_connector(*provider, window, cx)
                }
                app_views::AppView::Notes => self.open_notes(window, cx),
                app_views::AppView::Automations => self.open_automations(window, cx),
                app_views::AppView::Todos => self.open_todos(window, cx),
            },
            TitleTab::Session(id) => {
                let focus = self.workspace.tab_focus(*id);
                let review_focused = self.tab_review_focused(*id, cx);
                self.open(focus, window, cx);
                if review_focused
                    && self.workspace.active == Some(focus)
                    && let Some(review) = self.active_review(cx)
                {
                    self.review_focused = true;
                    review.update(cx, |r, cx| r.focus_active(window, cx));
                }
            }
            TitleTab::Bot { id, .. } => {
                if let Some(bots) = self.bots.clone() {
                    bots.update(cx, |m, cx| m.open(id, window, cx));
                }
            }
        }
    }
    pub(super) fn restore_scope_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let tab = self.workspace.scope_focus.get(&self.tab_scope()).cloned();
        if let Some(tab) = tab.filter(|tab| self.visible_tabs(cx).contains(tab)) {
            if matches!(tab, TitleTab::Bot { .. }) {
                self.workspace.active = None;
            }
            self.open_title_tab(&tab, window, cx);
            return true;
        }
        false
    }

    pub(super) fn run_title_tab_menu(
        &mut self,
        tab: TitleTab,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = self.visible_tabs(cx);
        let Some(index) = tabs.iter().position(|entry| *entry == tab) else {
            return;
        };
        if action == "close" {
            if let Some(id) = tab.session() {
                self.close(id, window, cx);
            } else {
                self.close_title_tabs(&[tab], None, window, cx);
            }
            return;
        }
        let closing = tabs
            .into_iter()
            .enumerate()
            .filter(|(ix, entry)| match action {
                "others" => *entry != tab,
                "right" => *ix > index,
                "left" => *ix < index,
                "all" => true,
                _ => false,
            })
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();
        self.close_title_tabs(&closing, (action == "others").then_some(tab), window, cx);
    }

    pub(super) fn close_title_tabs(
        &mut self,
        tabs: &[TitleTab],
        preferred: Option<TitleTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirm_terminal_tab_close(tabs, preferred.clone(), window, cx) {
            return;
        }
        self.close_title_tabs_confirmed(tabs, preferred, window, cx);
    }

    pub(super) fn close_title_tabs_confirmed(
        &mut self,
        tabs: &[TitleTab],
        preferred: Option<TitleTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.visible_tabs(cx);
        let closing: HashSet<_> = tabs
            .iter()
            .filter(|tab| self.workspace.title_order.contains(tab))
            .cloned()
            .collect();
        if closing.is_empty() {
            return;
        }
        let active = self.active_title_tab(cx);
        let replace = active.as_ref().is_some_and(|tab| closing.contains(tab));
        let next = if replace {
            preferred
                .filter(|tab| before.contains(tab) && !closing.contains(tab))
                .or_else(|| {
                    let index = before.iter().position(|tab| Some(tab) == active.as_ref())?;
                    // Browsers select the next tab to the right, then the one to the left.
                    before[index + 1..]
                        .iter()
                        .chain(before[..index].iter().rev())
                        .find(|tab| !closing.contains(*tab))
                        .cloned()
                })
        } else {
            None
        };
        self.navigation_revision += 1;
        self.close_menu(cx);
        self.tab_drag = None;
        self.tab_shifts.clear();
        self.tab_hover = None;
        self.tab_scroll_task = None;
        self.cancel_pane_drag(cx);
        let session_active = self.workspace.active;
        for tab in &closing {
            if let TitleTab::Terminal { id, .. } = tab {
                if self.active_terminal_tab.as_ref() == Some(id) {
                    self.leave_app_views(cx);
                }
                self.terminal_tabs.remove(id);
            }
            if let TitleTab::App(view) = tab
                && self.active_app_view == Some(*view)
            {
                self.leave_app_views(cx);
            }
            if *tab == TitleTab::Search {
                self.leave_app_search(cx);
                self.saved_search = None;
            }
            if let TitleTab::Session(id) = tab {
                self.workspace.close(*id);
            }
        }
        self.workspace
            .title_order
            .retain(|tab| !closing.contains(tab));
        self.workspace
            .scope_focus
            .retain(|_, tab| !closing.contains(tab));
        if replace {
            if let Some(bots) = self.bots.clone() {
                bots.update(cx, |m, cx| m.close(cx));
            }
            self.workspace.active = None;
            self.review_focused = false;
            self.review_slide = None;
            self.pane_motion = None;
            if let Some(tab) = next {
                self.open_title_tab(&tab, window, cx);
            } else {
                self.ensure_project_session(window, cx);
            }
        } else {
            self.workspace.active =
                session_active.filter(|id| self.workspace.tab_of(*id).is_some());
        }
        self.history
            .prune(&self.workspace.open_sessions(), self.workspace.active);
        if self.workspace.active.is_none() && !self.bot_active(cx) && !self.app_view_open() {
            self.focus_active(window, cx);
        }
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn reorder_title_tab(&mut self, tab: &TitleTab, to: usize, cx: &App) {
        let mut visible = self.visible_tabs(cx);
        let Some(from) = visible.iter().position(|entry| entry == tab) else {
            return;
        };
        let entry = visible.remove(from);
        visible.insert(to.min(visible.len()), entry);
        let set: HashSet<_> = visible.iter().cloned().collect();
        let mut ordered = visible.into_iter();
        for entry in &mut self.workspace.title_order {
            if set.contains(entry) {
                *entry = ordered.next().unwrap();
            }
        }
        self.workspace.tabs = self
            .workspace
            .title_order
            .iter()
            .filter_map(TitleTab::session)
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{shell::tests::window, workspace::AppView};
    use gpui::TestAppContext;

    #[test]
    #[ignore = "Requires Metal and VOKKY_CONNECTOR_CAPTURE_DIR"]
    fn connector_native_navigation_capture() {
        let output =
            std::path::PathBuf::from(std::env::var_os("VOKKY_CONNECTOR_CAPTURE_DIR").unwrap());
        assert!(output.is_absolute() && output.is_dir());
        let store = Arc::new(Mutex::new(
            vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let session = locked(&store)
            .unwrap()
            .create("codex", "/work/connector-fixture")
            .unwrap();
        let mut state = Workspace::default();
        state.open(session.id);
        let saved = serde_json::to_string(&state).unwrap();
        let mut cx = gpui::HeadlessAppContext::with_platform(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(crate::assets::Assets),
            || {
                Ok(Some(Box::new(
                    gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
                )))
            },
        );
        vorkspace_ui::clock::use_test_clock(&cx);
        let window = cx
            .open_window(gpui::size(px(1280.), px(800.)), |_, cx| {
                cx.new(|cx| {
                    Shell::new(
                        store,
                        vec![session],
                        "/work/connector-fixture".into(),
                        Some(saved),
                        cx,
                    )
                })
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(&mut cx, |shell, w, cx| {
                shell.ensure_background_inbox(w, cx);
                shell.open_connector(Connector::Github, w, cx);
                shell.open_connector(Connector::Jira, w, cx);
            })
            .unwrap();
        for (name, expanded) in [("expanded", true), ("compact", false)] {
            window
                .update(&mut cx, |shell, _, cx| {
                    shell.workspace.project_rail_open = expanded;
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("connectors-{name}.png")))
                .unwrap();
        }
        window
            .update(&mut cx, |shell, w, cx| {
                shell.open_settings(crate::settings::Section::Connectors, w, cx)
            })
            .unwrap();
        cx.run_until_parked();
        vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join("connectors-settings.png"))
            .unwrap();
    }
    #[gpui::test]
    fn command_w_closes_tabless_pages_and_restores_conversation(cx: &mut TestAppContext) {
        cx.update(|cx| {
            bind_keys(cx);
            crate::input::bind(cx);
            vorkspace_ui::clock::use_test_clock(cx);
        });
        let (window, _, session) = window(cx);
        let chat = window
            .update(cx, |shell, w, cx| {
                shell.open(session, w, cx);
                let chat = shell.sessions[&session].clone();
                chat.update(cx, |chat, cx| {
                    chat.busy = true;
                    chat.input.update(cx, |input, cx| {
                        input.set_text("unsent conversation draft".into(), cx)
                    });
                });
                shell.focus_active(w, cx);
                chat
            })
            .unwrap();
        for page in [
            TitleTab::Search,
            TitleTab::App(AppView::Connector(Connector::Github)),
            TitleTab::App(AppView::Connector(Connector::Gitlab)),
            TitleTab::App(AppView::Connector(Connector::Jira)),
            TitleTab::App(AppView::Notes),
            TitleTab::App(AppView::Automations),
        ] {
            let strip = window
                .update(cx, |shell, w, cx| {
                    let strip = shell.workspace.title_order.clone();
                    shell.open_title_tab(&page, w, cx);
                    if let Some(search) = &shell.app_search {
                        search
                            .view
                            .read(cx)
                            .input
                            .clone()
                            .update(cx, |input, cx| input.set_text("saved search".into(), cx));
                    }
                    strip
                })
                .unwrap();
            cx.run_until_parked();
            window
                .update(cx, |shell, _, cx| {
                    assert!(shell.tabless_page(cx), "{page:?}");
                    assert!(!shell.workspace.title_order.contains(&page));
                })
                .unwrap();
            cx.simulate_keystrokes(window.into(), "cmd-w");
            cx.run_until_parked();
            window
                .update(cx, |shell, w, cx| {
                    assert!(!shell.tabless_page(cx), "{page:?} must close");
                    assert_eq!(shell.active_title_tab(cx), Some(TitleTab::Session(session)));
                    assert_eq!(shell.workspace.title_order, strip);
                    assert_eq!(shell.sessions[&session], chat);
                    let chat = chat.read(cx);
                    assert!(chat.busy, "page close must not cancel parked work");
                    assert_eq!(chat.input.read(cx).content, "unsent conversation draft");
                    assert!(chat.input.focus_handle(cx).is_focused(w), "{page:?}");
                    if page == TitleTab::Search {
                        assert_eq!(shell.saved_search.as_ref().unwrap().0, "saved search");
                    }
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn search_opened_from_a_connector_restores_visible_conversation_focus(cx: &mut TestAppContext) {
        cx.update(|cx| {
            bind_keys(cx);
            crate::input::bind(cx);
        });
        let (window, _, session) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open_connector(Connector::Github, w, cx)
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| shell.open_app_search(w, cx))
            .unwrap();
        cx.run_until_parked();
        cx.simulate_keystrokes(window.into(), "cmd-w");
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                assert!(!shell.tabless_page(cx));
                assert_eq!(shell.workspace.active, Some(session));
                assert!(
                    shell.sessions[&session]
                        .read(cx)
                        .input
                        .focus_handle(cx)
                        .is_focused(w)
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn command_w_closes_channels_without_closing_parked_session(cx: &mut TestAppContext) {
        cx.update(|cx| {
            bind_keys(cx);
            crate::input::bind(cx);
        });
        let (window, _, session) = window(cx);
        let strip = window
            .update(cx, |shell, _, cx| {
                shell.workspace.preferences.channels = true;
                shell.apply_preferences(cx);
                shell.invalidate_chrome(cx);
                cx.notify();
                shell.workspace.title_order.clone()
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let add = visual.debug_bounds("classic-add-channel").unwrap();
        visual.simulate_click(add.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert!(shell.bot_manager().read(cx).classic_channels_open())
            })
            .unwrap();
        visual.simulate_keystrokes("cmd-w");
        visual.run_until_parked();
        window
            .update(visual, |shell, w, cx| {
                assert!(!shell.bot_active(cx));
                assert_eq!(shell.workspace.title_order, strip);
                assert_eq!(shell.workspace.active, Some(session));
                assert!(
                    shell.sessions[&session]
                        .read(cx)
                        .input
                        .focus_handle(cx)
                        .is_focused(w)
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn command_w_parks_bot_work_and_draft_and_restores_conversation(cx: &mut TestAppContext) {
        cx.update(|cx| {
            bind_keys(cx);
            crate::input::bind(cx);
            vorkspace_ui::clock::use_test_clock(cx);
        });
        let (window, _, session) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.bot_manager().update(cx, |bots, cx| {
                    bots.create_named(vec!["/work/vorkspace".into()], w, cx)
                });
            })
            .unwrap();
        cx.run_until_parked();
        let (bot, chat) = window
            .update(cx, |shell, _, cx| {
                let manager = shell.bot_manager();
                let bots = manager.read(cx);
                let id = bots.active.clone().unwrap();
                let chat = bots.chat().unwrap();
                chat.update(cx, |chat, cx| {
                    chat.busy = true;
                    chat.input.update(cx, |input, cx| {
                        input.set_text("unsent bot draft".into(), cx)
                    });
                });
                (id, chat)
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "cmd-w");
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                assert!(!shell.bot_active(cx));
                assert_eq!(shell.active_title_tab(cx), Some(TitleTab::Session(session)));
                assert!(
                    shell.sessions[&session]
                        .read(cx)
                        .input
                        .focus_handle(cx)
                        .is_focused(w)
                );
                assert!(chat.read(cx).busy);
                assert_eq!(chat.read(cx).input.read(cx).content, "unsent bot draft");
                shell.open_title_tab(&shell.bot_tab(bot), w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.bot_manager().read(cx).chat(), Some(chat.clone()));
                assert!(chat.read(cx).busy);
                assert_eq!(chat.read(cx).input.read(cx).content, "unsent bot draft");
            })
            .unwrap();
    }

    #[gpui::test]
    fn rapid_new_tabs_follow_request_order(cx: &mut TestAppContext) {
        let (window, _, first) = window(cx);
        let providers = [Kind::Claude, Kind::Codex, Kind::Claude, Kind::Codex];
        window
            .update(cx, |shell, _, cx| {
                for provider in providers {
                    shell.create(provider, cx);
                }
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let tabs = shell.visible_tabs(cx);
                assert_eq!(tabs.len(), 5);
                assert_eq!(tabs[0], TitleTab::Session(first));
                for (tab, provider) in tabs[1..].iter().zip(providers) {
                    assert_eq!(
                        shell.sessions[&tab.session().unwrap()]
                            .read(cx)
                            .summary
                            .provider,
                        provider.id()
                    );
                }
                assert_eq!(shell.active_title_tab(cx).as_ref(), tabs.last());
            })
            .unwrap();
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use crate::shell::tests::window;
    use gpui::{Modifiers, TestAppContext, point};

    /// The selected rail row is painted by `crate::liquid`, not as a quad.
    fn pill_at(window: gpui::WindowHandle<Shell>, key: &str, cx: &mut TestAppContext) -> bool {
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                shell.liquid.selected().as_deref() == Some(key)
            })
            .unwrap()
    }
    fn sidebar_highlight(
        window: gpui::WindowHandle<Shell>,
        selector: String,
        cx: &mut TestAppContext,
    ) -> bool {
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let selector = Box::leak(selector.into_boxed_str());
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("the sidebar row {selector} is visible"));
        window
            .update(visual, |_, window, _| {
                let scale = window.scale_factor();
                window.painted_quads().into_iter().any(|quad| {
                    (quad.bounds.left().0 - f32::from(bounds.left()) * scale).abs() < 1.
                        && (quad.bounds.top().0 - f32::from(bounds.top()) * scale).abs() < 1.
                        && (quad.bounds.size.width.0 - f32::from(bounds.size.width) * scale).abs()
                            < 1.
                        && (quad.bounds.size.height.0 - f32::from(bounds.size.height) * scale).abs()
                            < 1.
                        && quad
                            .background
                            .as_solid()
                            .is_some_and(|color| (color.a - 0.1).abs() < 0.001)
                })
            })
            .unwrap()
    }

    #[gpui::test]
    fn sidebar_highlights_only_the_visible_bot_or_session(cx: &mut TestAppContext) {
        let (window, other, session) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                for id in [other, session] {
                    shell.sessions[&id].update(cx, |view, _| {
                        view.summary.cwd = "/work/vorkspace".into();
                        view.summary.has_messages = true;
                    });
                }
                shell.bot_manager().update(cx, |m, cx| {
                    m.create_named(vec!["/work/vorkspace".into()], w, cx)
                });
            })
            .unwrap();
        cx.run_until_parked();
        let bot = window
            .update(cx, |shell, w, cx| {
                let bot = shell.active_title_tab(cx).unwrap();
                shell.open(session, w, cx);
                bot
            })
            .unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        let TitleTab::Bot { id, .. } = &bot else {
            panic!("expected a Bot");
        };
        let bot_row = format!("bot-rail-{id}");
        let bot_key = format!("bot:{id}");
        let session_row = format!("sidebar-session-{session}");
        let project_row = window
            .update(cx, |shell, _, _| {
                let project = shell
                    .workspace
                    .projects
                    .iter()
                    .position(|project| project == "/work/vorkspace")
                    .unwrap();
                format!("classic-project-{project}")
            })
            .unwrap();
        assert!(sidebar_highlight(window, session_row.clone(), cx));
        assert!(!pill_at(window, &bot_key, cx));
        let click = |selector: &str, cx: &mut TestAppContext| {
            let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
            let bounds = visual
                .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
                .unwrap();
            visual.simulate_click(bounds.center(), Default::default());
            visual.simulate_mouse_move(point(px(5.), px(5.)), None, Default::default());
            visual.run_until_parked();
            visual.refresh().unwrap();
        };
        click(&bot_row, cx);
        assert!(pill_at(window, &bot_key, cx));
        assert!(
            gpui::VisualTestContext::from_window(window.into(), cx)
                .debug_bounds(Box::leak(session_row.clone().into_boxed_str()))
                .is_none(),
            "the original Bot view hides the parked session pane"
        );
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.workspace.active, Some(session));
                assert_eq!(shell.workspace.project, "/work/vorkspace");
                assert_ne!(
                    shell.workspace.scope_focus.get(&shell.tab_scope()),
                    Some(&bot),
                    "a Bot page is never remembered as a tab"
                );
            })
            .unwrap();
        click(&project_row, cx);
        window
            .update(cx, |shell, _, cx| {
                assert!(!shell.bot_active(cx));
                assert_eq!(shell.workspace.project, "/work/vorkspace");
                assert_eq!(shell.workspace.active, Some(session));
                assert_eq!(
                    shell.workspace.project_focus.get("/work/vorkspace"),
                    Some(&session)
                );
                assert_eq!(shell.active_title_tab(cx), Some(TitleTab::Session(session)));
            })
            .unwrap();
        assert!(sidebar_highlight(window, session_row.clone(), cx));
        assert!(!pill_at(window, &bot_key, cx));
        // Opening from the title strip must also clear an old bulk selection.
        window
            .update(cx, |shell, w, cx| {
                for id in [other, session] {
                    shell.select_sidebar_session(
                        id,
                        Modifiers {
                            platform: true,
                            ..Default::default()
                        },
                        w,
                        cx,
                    );
                }
                assert_eq!(shell.selected_sessions.len(), 2);
                shell.open_title_tab(&bot, w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.selected_sessions.is_empty());
                assert!(shell.selection_anchor.is_none());
                assert!(shell.sidebar_target.is_none());
            })
            .unwrap();
        assert!(pill_at(window, &bot_key, cx));
        for selector in [&session_row, &format!("sidebar-session-{other}")] {
            assert!(
                gpui::VisualTestContext::from_window(window.into(), cx)
                    .debug_bounds(Box::leak(selector.clone().into_boxed_str()))
                    .is_none(),
                "the Bot surface hides session row {selector}"
            );
        }
        click(&project_row, cx);
        assert!(sidebar_highlight(window, session_row, cx));
        assert!(!pill_at(window, &bot_key, cx));
        assert!(!sidebar_highlight(
            window,
            format!("sidebar-session-{other}"),
            cx
        ));
    }
}
