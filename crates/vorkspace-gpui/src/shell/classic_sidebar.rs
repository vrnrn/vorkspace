//! The original workspace pane: project header, Sessions / Explorer / Changes,
//! and a session filter. App navigation and projects remain in the left rail.
use super::*;
use crate::{right_panel::RightTab, workspace::SidebarTab};

impl Shell {
    pub(super) fn classic_tool_tab(&self) -> Option<RightTab> {
        match self.workspace.sidebar_tab() {
            SidebarTab::Sessions | SidebarTab::Todos => None,
            SidebarTab::Files => Some(RightTab::Files),
            SidebarTab::Changes => Some(RightTab::Changes),
            SidebarTab::Graph => Some(RightTab::Graph),
        }
    }

    pub(super) fn classic_tab_for_tool(tab: RightTab) -> SidebarTab {
        match tab {
            RightTab::Files => SidebarTab::Files,
            RightTab::Changes => SidebarTab::Changes,
            RightTab::Graph => SidebarTab::Graph,
        }
    }

    pub(super) fn select_classic_sidebar(
        &mut self,
        tab: SidebarTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_app_views(cx);
        self.leave_app_search(cx);
        self.close_preview(cx);
        if let Some(bots) = &self.bots {
            bots.update(cx, |bots, cx| bots.close(cx));
        }
        self.workspace
            .sidebar_tabs
            .insert(self.workspace.project.clone(), tab);
        self.sidebar_auto_hidden = false;
        self.set_sidebar_visible(true, cx);
        self.ensure_right_panel(window, cx);
        self.sync_right_panel(cx);
        if tab == SidebarTab::Sessions {
            window.focus(&self.sidebar_focus, cx);
        } else if tab == SidebarTab::Todos {
            if let Some(board) = &self.project_todos {
                window.focus(&board.focus_handle(cx), cx);
            }
        } else if let Some(panel) = &self.right_panel {
            panel.update(cx, |panel, cx| panel.focus_pane(window, cx));
        }
        self.invalidate_chrome(cx);
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn classic_workspace_header(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("classic-workspace-header")
            .debug_selector(|| "classic-workspace-header".into())
            .h(px(TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            .border_b_1()
            .border_color(crate::right_panel::stroke())
            .child(
                div()
                    .id("classic-workspace-project")
                    .debug_selector(|| "sidebar-project-button".into())
                    .role(Role::Button)
                    .aria_label("Switch project")
                    .focusable()
                    .tab_stop(true)
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(28.))
                    .rounded_md()
                    .hover(|s| s.bg(content(0.08)))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(self.project_label(&self.workspace.project)),
                    )
                    .child(icon("chevron-down", 12.).text_color(content(0.45)))
                    .child(self.project_anchor.probe())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_menu(
                            MenuTarget::Header,
                            Anchor::Slot(this.project_anchor.clone()),
                            window,
                            cx,
                        );
                    })),
            )
            // The project's diff stat lives here while the sidebar is open.
            .children(self.project_diff_stat(cx))
            .child(
                icon_button("classic-find-file", "Search files", "search")
                    .debug_selector(|| "sidebar-file-search".into())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_quick_open(false, window, cx)),
                    ),
            )
            .child(
                icon_button("classic-new-session", "New session", "plus")
                    .debug_selector(|| "classic-new-session".into())
                    .on_click(cx.listener(|this, _, _, cx| this.create(this.default_kind(), cx))),
            )
            .into_any()
    }

    pub(super) fn classic_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.refresh_sidebar_structure(cx);
        let tab = self.workspace.sidebar_tab();
        let tabs = SidebarTab::normalize(&self.workspace.sidebar_tab_order);
        let mut body = div()
            .id("classic-workspace-sidebar")
            .debug_selector(|| "sidebar-content".into())
            .key_context("SessionSidebar")
            .track_focus(&self.sidebar_focus)
            .on_action(
                cx.listener(|this, _: &session_groups::RenameSidebarItem, window, cx| {
                    this.rename_sidebar_item(window, cx)
                }),
            )
            .on_action(cx.listener(
                |this, _: &session_groups::DeleteSidebarSelection, window, cx| {
                    this.delete_sidebar_selection(window, cx)
                },
            ))
            .w(px(self.workspace.right_panel_width))
            .h_full()
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .id("classic-workspace-tabs")
                    .debug_selector(|| "classic-workspace-tabs".into())
                    .role(Role::TabList)
                    .aria_label("Workspace")
                    .h(px(36.))
                    .flex_shrink_0()
                    .relative()
                    .flex()
                    // Each tab's pill runs from its top down to the line.
                    .items_end()
                    .gap_px()
                    .px_2()
                    .child(self.sidebar_tab_liquid.surface(None, false))
                    .children(tabs.into_iter().map(|item| {
                        let selected = tab == item
                            || (tab == SidebarTab::Graph && item == SidebarTab::Changes);
                        let id: SharedString =
                            format!("classic-tab-{}", item.label().to_lowercase()).into();
                        div()
                            .id(id.clone())
                            .debug_selector(move || id.to_string())
                            .role(Role::Tab)
                            .aria_label(item.label())
                            .aria_selected(selected)
                            .focusable()
                            .tab_stop(true)
                            .h(px(30.))
                            .flex_1()
                            .min_w_0()
                            .relative()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_t_md()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(if selected { 0.9 } else { 0.5 }))
                            .hover(|s| s.text_color(content(1.)))
                            .focus(|s| s.text_color(content(1.)))
                            .child(
                                self.sidebar_tab_liquid
                                    .row(format!("sidebar-tab:{}", item.label()), selected),
                            )
                            .child(item.label())
                            .on_hover(cx.listener(|this, _, _, cx| {
                                this.sidebar_frame.update(cx, |_, cx| cx.notify())
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_classic_sidebar(item, window, cx)
                            }))
                    })),
            );
        if tab == SidebarTab::Sessions {
            body = body
                .child(
                    div()
                        .id("classic-session-filter")
                        .debug_selector(|| "classic-session-filter".into())
                        .h(px(36.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .border_b_1()
                        .border_color(crate::right_panel::stroke())
                        .child(icon("search", 12.).text_color(content(0.5)))
                        .child(div().flex_1().min_w_0().child(self.sidebar_search.clone()))
                        .child(self.sidebar_filter_button(cx)),
                )
                .child(self.virtual_sidebar(cx));
        } else if tab == SidebarTab::Todos {
            body = body.child(self.project_todo_board(cx));
        } else if let Some(panel) = &self.right_panel {
            if matches!(tab, SidebarTab::Changes | SidebarTab::Graph) {
                body = body.child(
                    div()
                        .h(px(30.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .children(
                            [
                                (SidebarTab::Changes, "Changes"),
                                (SidebarTab::Graph, "History"),
                            ]
                            .into_iter()
                            .map(|(item, label)| {
                                let id = format!("classic-source-{}", label.to_lowercase());
                                div()
                                    .id(SharedString::from(id.clone()))
                                    .debug_selector(move || id.clone())
                                    .role(Role::Tab)
                                    .aria_label(label)
                                    .aria_selected(tab == item)
                                    .focusable()
                                    .tab_stop(true)
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_size(px(11.))
                                    .when(tab == item, |d| d.bg(content(0.1)))
                                    .hover(|s| s.bg(content(0.08)))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.select_classic_sidebar(item, window, cx)
                                    }))
                            }),
                        ),
                );
            }
            body = body.child(div().flex_1().min_h_0().child(panel.clone()));
        }
        body.into_any()
    }

    pub(super) fn classic_session_preview(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let preview = self.session_preview.as_ref()?;
        let id = preview.id;
        Some(
            popover("session-preview", Anchor::Slot(preview.anchor.clone()))
                .side(Side::Left)
                .gap(8.)
                .width(260.)
                .max_height(440.)
                .track(preview.surface.clone())
                .child(
                    div()
                        .id("session-preview-content")
                        .role(Role::Tooltip)
                        .max_h(px(438.))
                        .overflow_y_scroll()
                        .on_hover(cx.listener(move |this, hovered, window, cx| {
                            if this
                                .session_preview
                                .as_ref()
                                .is_none_or(|preview| preview.id != id)
                            {
                                return;
                            }
                            if *hovered {
                                this.preview_close = None;
                            } else {
                                this.schedule_preview_close(window, cx);
                            }
                        }))
                        .child(self.sessions.get(&id).map_or_else(
                            || {
                                session_preview::render(
                                    self.session_metadata(id, cx).summary,
                                    preview.data.as_ref(),
                                    self.unseen_finished.contains(&id),
                                )
                            },
                            |view| view.read(cx).preview(self.unseen_finished.contains(&id)),
                        )),
                )
                .into_any_element(),
        )
    }
}
