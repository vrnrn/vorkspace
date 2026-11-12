//! The original workspace pane: project header, Sessions / Explorer / Changes,
//! and a session filter. App navigation and projects remain in the left rail.
use super::*;
use crate::{panel_kit::RightTab, workspace::SidebarTab};
use vorkspace_ui::radius;

impl Shell {
    pub(super) fn classic_tool_tab(&self) -> Option<RightTab> {
        match self.workspace.sidebar_tab() {
            SidebarTab::Sessions | SidebarTab::Todos => None,
            SidebarTab::Files => Some(RightTab::Files),
            SidebarTab::Changes => Some(RightTab::Changes),
            SidebarTab::Graph => Some(RightTab::Graph),
        }
    }

    pub(super) fn select_classic_sidebar(
        &mut self,
        tab: SidebarTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if tab == SidebarTab::Todos && !self.workspace.preferences.todos {
            return;
        }
        self.leave_app_views(cx);
        self.leave_app_search(cx);
        self.close_preview(cx);
        if let Some(bots) = &self.bots {
            bots.update(cx, |bots, cx| bots.close(cx));
        }
        self.workspace
            .sidebar_tabs
            .insert(self.workspace.project.clone(), tab);
        self.set_sidebar_visible(true, cx);
        self.ensure_right_panel(window, cx);
        self.sync_right_panel(cx);
        if tab == SidebarTab::Todos {
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
            // The name's hover fill lines up with the tab pills below it.
            .px_2()
            .border_b_1()
            .border_color(crate::panel_kit::stroke())
            .child(
                div()
                    .id("classic-workspace-project")
                    .debug_selector(|| "sidebar-project-button".into())
                    .role(Role::Button)
                    .aria_label("Switch project")
                    .map(vorkspace_ui::theme::tab_reachable)
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(28.))
                    .px_2()
                    .rounded(px(radius::CONTROL))
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .when(self.workspace.project.is_empty(), |d| {
                                d.text_color(content(0.5)).font_weight(FontWeight::NORMAL)
                            })
                            .child(if self.workspace.project.is_empty() {
                                "No project".into()
                            } else {
                                self.project_label(&self.workspace.project)
                            }),
                    )
                    .child(icon("chevron-down", 12.).text_color(content(0.45)))
                    .child(self.sidebar.project_anchor.probe())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_menu(
                            MenuTarget::Header,
                            Anchor::Slot(this.sidebar.project_anchor.clone()),
                            window,
                            cx,
                        );
                    })),
            )
            .when(!self.workspace.project.is_empty(), |d| {
                d.child(
                    icon_button(
                        "sidebar-project-actions",
                        "Project actions",
                        "more-horizontal",
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_menu(
                            MenuTarget::Project {
                                path: this.workspace.project.clone(),
                                rail: false,
                            },
                            Anchor::Slot(this.sidebar.project_anchor.clone()),
                            window,
                            cx,
                        );
                    })),
                )
            })
            .into_any()
    }

    pub(super) fn classic_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.workspace.sidebar_tab();
        let tabs = SidebarTab::normalize(&self.workspace.sidebar_tab_order);
        let todos_enabled = self.workspace.preferences.todos;
        let compact = self.sidebar.content_width < 200.;
        let mut body = div()
            .id("classic-workspace-sidebar")
            .debug_selector(|| "sidebar-content".into())
            .w_full()
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
                    .border_b_1()
                    .border_color(crate::panel_kit::stroke())
                    .child({
                        let items: Vec<SidebarTab> = tabs
                            .into_iter()
                            .filter(|item| *item != SidebarTab::Todos || todos_enabled)
                            .collect();
                        let selected_index = items.iter().position(|item| {
                            tab == *item
                                || (tab == SidebarTab::Graph && *item == SidebarTab::Changes)
                        });
                        let listener = cx.entity().downgrade();
                        vorkspace_ui::glide::GlideList::new(
                            "classic-workspace-tab-glide",
                            selected_index,
                            div().size_full().flex().items_center().gap_px().px_2(),
                            content(0.1),
                            radius::CONTROL,
                            move |glide| {
                                let ready = glide.ready();
                                items
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, item)| {
                                        let selected = selected_index == Some(index);
                                        let listener = listener.clone();
                                        let hovered = listener.clone();
                                        let id: SharedString =
                                            format!("classic-tab-{}", item.label().to_lowercase())
                                                .into();
                                        div()
                                            .id(id.clone())
                                            .debug_selector(move || id.to_string())
                                            .role(Role::Tab)
                                            .aria_label(item.label())
                                            .aria_selected(selected)
                                            .map(vorkspace_ui::theme::tab_reachable)
                                            .h(px(28.))
                                            .flex_1()
                                            .min_w_0()
                                            .relative()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(radius::CONTROL))
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(content(if selected { 0.9 } else { 0.5 }))
                                            // The glide's thumb draws the selection once it
                                            // has measured the strip.
                                            .when(selected && !ready, |d| d.bg(content(0.1)))
                                            .when(!selected, |d| {
                                                d.hover(|s| {
                                                    s.bg(vorkspace_ui::fluid::hover(0.08))
                                                        .text_color(content(1.))
                                                })
                                            })
                                            .child(glide.probe(index))
                                            .child(if compact {
                                                let mark = match item {
                                                    SidebarTab::Files => "folder",
                                                    SidebarTab::Changes => "git-compare",
                                                    SidebarTab::Todos => "check-circle",
                                                    SidebarTab::Graph => "clock",
                                                    SidebarTab::Sessions => "message-square",
                                                };
                                                icon(mark, 14.).into_any_element()
                                            } else {
                                                item.label().into_any_element()
                                            })
                                            .when(compact, |d| {
                                                d.tooltip(move |_, cx| {
                                                    cx.new(|_| {
                                                        crate::controls::Tooltip(
                                                            item.label().into(),
                                                        )
                                                    })
                                                    .into()
                                                })
                                            })
                                            // Keep the cached sidebar's hover fill current,
                                            // including clicks that arrive without a mouse move.
                                            .on_hover(move |_, _, cx| {
                                                let _ =
                                                    hovered.update(cx, |this: &mut Shell, cx| {
                                                        this.sidebar_frame
                                                            .update(cx, |_, cx| cx.notify())
                                                    });
                                            })
                                            .on_click(move |_, window, cx| {
                                                let _ =
                                                    listener.update(cx, |this: &mut Shell, cx| {
                                                        this.select_classic_sidebar(
                                                            item, window, cx,
                                                        )
                                                    });
                                            })
                                            .into_any_element()
                                    })
                                    .collect()
                            },
                        )
                    }),
            );
        if self.workspace.project.is_empty() {
            // Files, changes and a project board all belong to a project;
            // without one, each tab offers the same way in.
            body = body.child(
                crate::controls::page::empty(
                    icon("folder-open", 20.),
                    "No project open",
                    match tab {
                        SidebarTab::Files => "Open a project to browse its files.",
                        SidebarTab::Todos => "Open a project to plan its work.",
                        _ => "Open a project to review and commit its changes.",
                    },
                )
                .child(
                    crate::controls::page::secondary(
                        "sidebar-open-project",
                        "Open project",
                        false,
                        false,
                    )
                    .debug_selector(|| "sidebar-open-project".into())
                    .mt(px(12.))
                    .child("Open project…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.workspace.vorkspace_projects().is_empty() {
                            this.open_project(cx);
                        } else {
                            this.open_menu(
                                MenuTarget::Header,
                                Anchor::Slot(this.sidebar.project_anchor.clone()),
                                window,
                                cx,
                            );
                        }
                    })),
                ),
            );
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
                                (SidebarTab::Changes, "Changes", "git-compare"),
                                (SidebarTab::Graph, "History", "clock"),
                            ]
                            .into_iter()
                            .map(|(item, label, mark)| {
                                let id = format!("classic-source-{}", label.to_lowercase());
                                div()
                                    .id(SharedString::from(id.clone()))
                                    .debug_selector(move || id.clone())
                                    .role(Role::Tab)
                                    .aria_label(label)
                                    .aria_selected(tab == item)
                                    .map(vorkspace_ui::theme::tab_reachable)
                                    .px_2()
                                    .py_1()
                                    .rounded(px(radius::CONTROL))
                                    .text_size(px(11.))
                                    .when(tab == item, |d| d.bg(content(0.1)))
                                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
                                    .when(compact, |d| {
                                        d.child(icon(mark, 14.)).tooltip(move |_, cx| {
                                            cx.new(|_| crate::controls::Tooltip(label.into()))
                                                .into()
                                        })
                                    })
                                    .when(!compact, |d| d.child(label))
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
        let preview = self.preview.session_preview.as_ref()?;
        let id = preview.id;
        Some(
            // Sessions are listed in the rail at the window's left edge.
            popover("session-preview", Anchor::Slot(preview.anchor.clone()))
                .side(Side::Right)
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
                                .preview
                                .session_preview
                                .as_ref()
                                .is_none_or(|preview| preview.id != id)
                            {
                                return;
                            }
                            if *hovered {
                                this.preview.close = None;
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
