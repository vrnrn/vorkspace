//! The rail's navigation rows (Search, GitHub, Notes, …) and their options.
use super::*;
use vorkspace_ui::radius;

impl Shell {}

impl Shell {
    fn available_sidebar_navigation(&self, cx: &App) -> Vec<SidebarNavigation> {
        let enabled = self.enabled_connectors(cx);
        SidebarNavigation::normalize(&self.workspace.sidebar_navigation_order)
            .into_iter()
            .filter(|item| match item {
                SidebarNavigation::Connector(provider) => enabled.contains(provider),
                SidebarNavigation::Codebase => {
                    crate::codebase::enabled(&self.workspace.project, cx)
                }
                _ => true,
            })
            .collect()
    }

    pub(super) fn visible_sidebar_navigation(&self, cx: &App) -> Vec<SidebarNavigation> {
        self.available_sidebar_navigation(cx)
            .into_iter()
            .filter(|item| match item {
                SidebarNavigation::Connector(provider) => !self
                    .workspace
                    .sidebar_hidden_connectors
                    .contains(provider.id()),
                SidebarNavigation::Notes => {
                    self.workspace.preferences.notes && self.workspace.sidebar_notes_visible
                }
                SidebarNavigation::Automations => self.workspace.sidebar_automations_visible,
                SidebarNavigation::Codebase => true,
                SidebarNavigation::Todos => {
                    self.workspace.preferences.todos && self.workspace.sidebar_todos_visible
                }
            })
            .collect()
    }

    pub(super) fn sidebar_options_menu_items(&self, cx: &App) -> Vec<MenuItem> {
        let mut items: Vec<_> = self
            .available_sidebar_navigation(cx)
            .into_iter()
            .filter(|item| *item != SidebarNavigation::Codebase)
            .map(|item| {
                let visible = match item {
                    SidebarNavigation::Connector(provider) => !self
                        .workspace
                        .sidebar_hidden_connectors
                        .contains(provider.id()),
                    SidebarNavigation::Notes => self.workspace.sidebar_notes_visible,
                    SidebarNavigation::Automations => self.workspace.sidebar_automations_visible,
                    SidebarNavigation::Codebase => true,
                    SidebarNavigation::Todos => self.workspace.sidebar_todos_visible,
                };
                MenuAction::new(format!("sidebar-{}", item.id()), item.label())
                    .icon(item.icon())
                    .toggle(visible)
                    .reorderable()
                    .disabled(match item {
                        SidebarNavigation::Notes => !self.workspace.preferences.notes,
                        SidebarNavigation::Todos => !self.workspace.preferences.todos,
                        _ => false,
                    })
                    .into()
            })
            .collect();
        items.extend([
            MenuItem::Separator,
            MenuAction::new("sidebar-bots", "Bots")
                .icon("ghost")
                .toggle(self.workspace.preferences.bots)
                .into(),
            MenuAction::new("sidebar-subscriptions", "Subscriptions")
                .icon("chart-square")
                .toggle(self.workspace.sidebar_subscriptions_visible)
                .into(),
        ]);
        items
    }

    pub(super) fn reorder_sidebar_navigation(
        &mut self,
        event: &crate::popover::MenuReordered,
        cx: &mut Context<Self>,
    ) {
        let source = event
            .source
            .strip_prefix("sidebar-")
            .and_then(SidebarNavigation::from_id);
        let target = event
            .target
            .strip_prefix("sidebar-")
            .and_then(SidebarNavigation::from_id);
        let (Some(source), Some(target)) = (source, target) else {
            return;
        };
        let available = self.available_sidebar_navigation(cx);
        if self
            .workspace
            .reorder_sidebar_navigation(source, target, event.after, &available)
        {
            self.persist(cx);
            cx.notify();
        }
    }

    pub(super) fn change_sidebar_option(&mut self, action: &str, cx: &mut Context<Self>) {
        if action == "sidebar-activity" {
            let shown = self.workspace.sidebar_activity_visible
                && self.workspace.preferences.working_agents;
            self.workspace.sidebar_activity_visible = !shown;
            // An explicit Show also enables an older saved Working agents opt-out.
            if !shown {
                self.workspace.preferences.working_agents = true;
            }
            self.sync_settings(cx);
            self.invalidate_chrome(cx);
            self.persist(cx);
            cx.notify();
            return;
        }
        if let Some(provider) = action.strip_prefix("sidebar-").and_then(Connector::from_id) {
            // This shortcut preference never reaches the connector Settings owner.
            if !self.enabled_connectors(cx).contains(&provider) {
                return;
            }
            if !self
                .workspace
                .sidebar_hidden_connectors
                .remove(provider.id())
            {
                self.workspace
                    .sidebar_hidden_connectors
                    .insert(provider.id().into());
            }
        } else {
            let value = match action {
                "sidebar-notes" if self.workspace.preferences.notes => {
                    &mut self.workspace.sidebar_notes_visible
                }
                "sidebar-automations" => &mut self.workspace.sidebar_automations_visible,
                "sidebar-todos" => &mut self.workspace.sidebar_todos_visible,
                "sidebar-bots" => &mut self.workspace.preferences.bots,
                "sidebar-subscriptions" => &mut self.workspace.sidebar_subscriptions_visible,
                _ => return,
            };
            *value = !*value;
        }
        if action == "sidebar-subscriptions" && !self.workspace.sidebar_subscriptions_visible {
            self.usage_footer.update(cx, |usage, cx| usage.hide(cx));
        }
        if action == "sidebar-bots" {
            self.apply_preferences(cx);
            self.sync_settings(cx);
        }
        self.close_preview(cx);
        self.persist(cx);
        cx.notify();
    }
    pub(super) fn sidebar_navigation(&mut self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w_full()
            .px_2()
            .pt_1()
            .pb_3()
            .flex()
            .flex_col()
            .gap_px()
            .child(
                div()
                    .id("rail-search")
                    .role(Role::Button)
                    .aria_label("Search")
                    .focusable()
                    .tab_stop(true)
                    .debug_selector(|| "sidebar-search-field".into())
                    .group("rail-search")
                    .w_full()
                    .h(px(32.))
                    .px_2()
                    .rounded(px(radius::CONTROL))
                    .relative()
                    .when(self.app_search.is_some(), |d| d.bg(content(0.1)))
                    .text_color(content(
                        if self.app_search.is_some() || self.sidebar_hover == Some("Search") {
                            1.
                        } else {
                            0.5
                        },
                    ))
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|s| {
                        s.bg(vorkspace_ui::fluid::hover(0.08))
                            .text_color(content(1.))
                    })
                    .focus(|s| s.bg(content(0.1)).text_color(content(1.)))
                    .on_hover(cx.listener(|this, hovered, _, cx| {
                        this.sidebar_hover = if *hovered { Some("Search") } else { None };
                        this.sidebar_frame.update(cx, |_, cx| cx.notify());
                        this.project_rail_frame.update(cx, |_, cx| cx.notify());
                    }))
                    .on_click(cx.listener(|this, _, window, cx| this.focus_search(window, cx)))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(14.))
                    .child(
                        icon("search", 16.)
                            .text_color(content(0.35))
                            .group_hover("rail-search", |s| s.text_color(content(0.7))),
                    )
                    .child(div().flex_1().min_w_0().child("Search")),
            )
            .child({
                // Gathered first: the glide builds the rows when it renders.
                let rows: Vec<_> = self
                    .visible_sidebar_navigation(cx)
                    .into_iter()
                    .map(|item| {
                        let label = item.label();
                        let unread = Connector::ALL
                            .into_iter()
                            .find(|provider| provider.label() == label)
                            .is_some_and(|provider| self.connector_unread(provider, cx));
                        (
                            label,
                            item.icon(),
                            self.app_view_label() == Some(label),
                            unread,
                            self.sidebar_hover == Some(label),
                        )
                    })
                    .collect();
                let selected_index = rows.iter().position(|row| row.2);
                let shell = cx.entity().downgrade();
                vorkspace_ui::glide::GlideList::new(
                    "sidebar-navigation-glide",
                    selected_index,
                    div().w_full().flex().flex_col().gap_px(),
                    content(0.1),
                    radius::CONTROL,
                    move |glide| {
                        let ready = glide.ready();
                        rows.into_iter()
                            .enumerate()
                            .map(|(index, (label, glyph, selected, unread, hovered))| {
                                let clicked = shell.clone();
                                let hovering = shell.clone();
                                div()
                                    .id(label)
                                    .debug_selector(move || format!("sidebar-navigation-{label}"))
                                    .role(Role::Button)
                                    .map(vorkspace_ui::theme::tab_reachable)
                                    .aria_label(if unread {
                                        format!("{label}, new items")
                                    } else {
                                        label.into()
                                    })
                                    .aria_selected(selected)
                                    .cursor(CursorStyle::Arrow)
                                    .group("rail-action")
                                    .h(px(32.))
                                    .px_2()
                                    .rounded(px(radius::CONTROL))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(content(if selected || hovered { 1. } else { 0.5 }))
                                    .relative()
                                    .child(glide.probe(index))
                                    // The glide's thumb draws the selection once measured.
                                    .when(selected && !ready, |d| d.bg(content(0.1)))
                                    .hover(|s| {
                                        s.bg(vorkspace_ui::fluid::hover(0.08))
                                            .text_color(content(1.))
                                    })
                                    .child(icon(glyph, 16.).text_color(content(if selected {
                                        0.7
                                    } else {
                                        0.35
                                    })))
                                    .child(div().flex_1().child(label))
                                    .when(unread, |row| {
                                        row.child(
                                            div().size(px(6.)).rounded_full().bg(accent_color()),
                                        )
                                    })
                                    .on_click(move |_, window, cx| {
                                        let _ =
                                            clicked.update(
                                                cx,
                                                |this: &mut Shell, cx| match label {
                                                    "GitHub" => this.open_connector(
                                                        Connector::Github,
                                                        window,
                                                        cx,
                                                    ),
                                                    "GitLab" => this.open_connector(
                                                        Connector::Gitlab,
                                                        window,
                                                        cx,
                                                    ),
                                                    "Jira" => this.open_connector(
                                                        Connector::Jira,
                                                        window,
                                                        cx,
                                                    ),
                                                    "Notes" => this.open_notes(window, cx),
                                                    "Codebase" => this.open_codebase(window, cx),
                                                    "Automations" => {
                                                        this.open_automations(window, cx)
                                                    }
                                                    "To-dos" => this.open_todos(window, cx),
                                                    _ => (),
                                                },
                                            );
                                    })
                                    .on_hover(move |hovered, _, cx| {
                                        let _ = hovering.update(cx, |this: &mut Shell, cx| {
                                            if *hovered {
                                                this.sidebar_hover = Some(label);
                                            } else if this.sidebar_hover == Some(label) {
                                                this.sidebar_hover = None;
                                            }
                                            this.sidebar_frame.update(cx, |_, cx| cx.notify());
                                            this.project_rail_frame.update(cx, |_, cx| cx.notify());
                                        });
                                    })
                                    .into_any_element()
                            })
                            .collect()
                    },
                )
            })
            .into_any()
    }
}

#[cfg(test)]
mod tests;
