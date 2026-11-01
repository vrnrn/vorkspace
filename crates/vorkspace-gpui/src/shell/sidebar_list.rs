//! Only construct and lay out sidebar cards near the viewport.
use super::*;

use gpui::{ListAlignment, ListState, list};
use std::cell::{Cell, RefCell};
use vorkspace_core::session_groups::Entry;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Pinned,
    Folders,
    Group(Entry),
    Gap,
    Session(i64),
    GroupedSession { id: i64, first: bool, last: bool },
    Empty,
}

pub(super) struct SidebarList {
    pub(super) state: ListState,
    rows: Vec<Row>,
    scope: String,
    structure_dirty: Cell<bool>,
    indexed_minute: u64,
    #[cfg(test)]
    structural_rebuilds: usize,
    /// Only anchors constructed for the last viewport, including nested rails.
    visible_anchors: RefCell<Vec<AnchorSlot>>,
    visible_rows: RefCell<Vec<usize>>,
}

impl SidebarList {
    pub(super) fn new(cx: &mut Context<Shell>) -> Self {
        let state =
            ListState::new(0, ListAlignment::Top, px(96.)).with_uniform_item_height(px(33.));
        let owner = cx.weak_entity();
        state.set_scroll_handler(move |_, _, cx| {
            let _ = owner.update(cx, |shell, cx| {
                shell.sidebar_scrolled(cx);
                // Historical rows are retained as cheap IDs, but hit targets
                // only belong to the viewport that was actually laid out.
                shell.sidebar_list.clear_bounds();
                shell.sidebar_frame.update(cx, |_, cx| cx.notify());
            });
        });
        Self {
            state,
            rows: Vec::new(),
            scope: String::new(),
            structure_dirty: Cell::new(true),
            indexed_minute: 0,
            #[cfg(test)]
            structural_rebuilds: 0,
            visible_anchors: RefCell::new(Vec::new()),
            visible_rows: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn invalidate_structure(&self) {
        self.structure_dirty.set(true);
    }

    pub(super) fn track_anchor(&self, anchor: AnchorSlot) {
        self.visible_anchors.borrow_mut().push(anchor);
    }

    fn clear_bounds(&self) {
        for anchor in self.visible_anchors.borrow_mut().drain(..) {
            anchor.set(None);
        }
    }

    fn update(&mut self, scope: String, rows: Vec<Row>) {
        if self.scope != scope {
            self.state.reset_with_uniform_height(rows.len(), px(33.));
            self.scope = scope;
        } else if self.rows != rows {
            // Keep the scroll anchor when sessions are added, regrouped or
            // reordered. Only replace the changed run of list items.
            let prefix = self
                .rows
                .iter()
                .zip(&rows)
                .take_while(|(a, b)| a == b)
                .count();
            let suffix = self.rows[prefix..]
                .iter()
                .rev()
                .zip(rows[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            self.state.splice(
                prefix..self.rows.len() - suffix,
                rows.len() - prefix - suffix,
            );
        }
        self.rows = rows;
    }
}

// Match sidebar-session-scroll: 40px below, and 28px above scrolled inset rows.
// Mask only the edge bands so the virtual list never needs a full-pane copy.
fn sidebar_edge_mask(state: ListState, inset: bool, begin: bool) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let offset = state.logical_scroll_top();
            let bottom_height = px(40.).min(bounds.size.height);
            let top_height = if inset && (offset.item_ix > 0 || offset.offset_in_item > px(0.)) {
                px(28.).min((bounds.size.height - bottom_height).max(px(0.)))
            } else {
                px(0.)
            };
            for (at_top, height) in [(true, top_height), (false, bottom_height)] {
                if height <= px(0.) {
                    continue;
                }
                let band = gpui::Bounds::new(
                    gpui::point(
                        bounds.left(),
                        if at_top {
                            bounds.top()
                        } else {
                            bounds.bottom() - height
                        },
                    ),
                    gpui::size(bounds.size.width, height),
                );
                window.paint_quad(gpui::fill(band, gpui::black()).border_color(
                    gpui_apple::metal_renderer::edge_fade_marker(
                        begin,
                        if at_top {
                            height.as_f32() * window.scale_factor()
                        } else {
                            0.
                        },
                        if at_top {
                            0.
                        } else {
                            height.as_f32() * window.scale_factor()
                        },
                    ),
                ));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

impl Shell {
    /// Session membership/order changes independently of scroll position and
    /// hover state. Keep the expensive project index until a data notification
    /// invalidates it; individual cards still read live metadata each render.
    pub(super) fn refresh_sidebar_structure(&mut self, cx: &mut Context<Self>) {
        let scope = self.sidebar_scope();
        let minute = vorkspace_core::time::now_ms() / 60_000;
        if self.sidebar_list.structure_dirty.replace(false)
            || self.sidebar_list.scope != scope
            || self.sidebar_list.indexed_minute != minute
        {
            // Date/time expiry refreshes membership without resetting the
            // project scope or scroll anchor.
            self.sidebar_list.indexed_minute = minute;
            #[cfg(test)]
            {
                self.sidebar_list.structural_rebuilds += 1;
            }
            let ids = self.sidebar_session_ids(cx);
            self.sync_rails(&ids, cx);
            let entries = self.session_entries(&ids, cx);
            let listed: HashSet<_> = ids.iter().copied().collect();
            self.session_row_bounds.retain(|id, _| listed.contains(id));
            for id in ids {
                self.session_row_bounds.entry(id).or_default();
            }
            let folders: HashSet<_> = entries
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Folder { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect();
            self.folder_bounds.retain(|id, _| folders.contains(id));
            for id in folders {
                self.folder_bounds.entry(id).or_default();
            }
            self.prepare_sidebar_list(&entries, cx);
        }
        self.sync_sidebar_list_visibility();
        self.sidebar_list.clear_bounds();
    }

    fn sidebar_scope(&self) -> String {
        format!(
            "{}\0{}\0{}\0{:?}\0{}\0{}",
            self.workspace.project,
            self.working_copy(),
            self.query,
            self.workspace.session_filters,
            self.workspace.preferences.show_archived,
            self.workspace.classic_layout
        )
    }

    pub(super) fn prepare_sidebar_list(&mut self, entries: &[Entry], _cx: &App) {
        let groups = self.project_groups();
        let mut rows = Vec::new();
        let mut folders = false;
        for entry in entries {
            let (members, collapsed) = match entry {
                Entry::Pinned { sessions } => {
                    // Pins read as ordinary cards under a plain label.
                    rows.push(Row::Pinned);
                    rows.extend(
                        sessions
                            .iter()
                            .filter(|id| !self.rails.children.contains(id))
                            .map(|id| Row::Session(*id)),
                    );
                    rows.push(Row::Gap);
                    continue;
                }
                Entry::Reminders {
                    sessions,
                    collapsed,
                } => (sessions, *collapsed),
                Entry::Folder { id, sessions } => {
                    (sessions, groups.folder(id).is_some_and(|g| g.collapsed))
                }
                Entry::Session(_) => continue,
            };
            if !folders {
                rows.push(Row::Folders);
                folders = true;
            }
            rows.push(Row::Group(entry.clone()));
            if !collapsed || !self.query.is_empty() {
                let members: Vec<_> = members
                    .iter()
                    .copied()
                    .filter(|id| !self.rails.children.contains(id))
                    .collect();
                rows.extend(
                    members
                        .iter()
                        .enumerate()
                        .map(|(ix, id)| Row::GroupedSession {
                            id: *id,
                            first: ix == 0,
                            last: ix + 1 == members.len(),
                        }),
                );
            }
        }
        if folders {
            rows.push(Row::Gap);
        }
        for entry in entries {
            if let Entry::Session(id) = entry
                && !self.rails.children.contains(id)
            {
                rows.push(Row::Session(*id));
            }
        }
        if entries.is_empty() {
            rows.push(Row::Empty);
        }
        self.sidebar_list.update(self.sidebar_scope(), rows);
    }

    fn sync_sidebar_list_visibility(&mut self) {
        // Open rails have variable height. Remeasure only materialized rows,
        // rather than scanning every session in the project every frame.
        for ix in self.sidebar_list.visible_rows.borrow_mut().drain(..) {
            if let Some(Row::Session(id) | Row::GroupedSession { id, .. }) =
                self.sidebar_list.rows.get(ix)
                && (self.rails.started.contains_key(id)
                    || self.orchestration_summaries.contains_key(id))
            {
                self.sidebar_list.state.remeasure_items(ix..ix + 1);
            }
        }
    }

    pub(super) fn virtual_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("sidebar-scroll")
            .debug_selector(|| "sidebar-sessions".into())
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|this, _, _, cx| this.sidebar_scrolled(cx)))
            .child(sidebar_edge_mask(
                self.sidebar_list.state.clone(),
                !self.workspace.classic_layout,
                true,
            ))
            .child(
                list(
                    self.sidebar_list.state.clone(),
                    cx.processor(|this, ix, _, cx| this.sidebar_list_row(ix, cx)),
                )
                .size_full(),
            )
            .child(sidebar_edge_mask(
                self.sidebar_list.state.clone(),
                !self.workspace.classic_layout,
                false,
            ))
            .into_any()
    }

    fn sidebar_list_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.sidebar_list.rows.get(ix).cloned() else {
            return div().into_any();
        };
        self.sidebar_list.visible_rows.borrow_mut().push(ix);
        let content = match row {
            Row::Pinned => section_label("Pinned").into_any(),
            Row::Folders => section_label("Folders").into_any(),
            Row::Group(entry) => {
                if let Entry::Folder { id, .. } = &entry
                    && let Some(anchor) = self.folder_bounds.get(id)
                {
                    self.sidebar_list.track_anchor(anchor.clone());
                }
                self.sidebar_group_header(&entry, cx)
            }
            Row::Gap => return div().h(px(12.)).into_any(),
            Row::Session(id) => self.session_card(id, cx),
            Row::GroupedSession { id, first, last } => {
                return div()
                    .px_2()
                    .child(
                        div()
                            .ml(px(10.))
                            .when(first, |d| d.pt_1())
                            .child(rails::step(last, self.nested_session_card(id, cx))),
                    )
                    .into_any();
            }
            Row::Empty => div()
                .px_2()
                .py_2()
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .child(if self.query.is_empty() {
                    "No sessions yet"
                } else {
                    "No matching sessions"
                })
                .into_any(),
        };
        div().px_2().pb_px().child(content).into_any()
    }
}

impl Shell {
    fn available_sidebar_navigation(&self, cx: &App) -> Vec<SidebarNavigation> {
        let enabled = self.enabled_connectors(cx);
        SidebarNavigation::normalize(&self.workspace.sidebar_navigation_order)
            .into_iter()
            .filter(|item| match item {
                SidebarNavigation::Connector(provider) => enabled.contains(provider),
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
                SidebarNavigation::Todos => self.workspace.sidebar_todos_visible,
            })
            .collect()
    }

    pub(super) fn sidebar_options_menu_items(&self, cx: &App) -> Vec<MenuItem> {
        let mut items: Vec<_> = self
            .available_sidebar_navigation(cx)
            .into_iter()
            .map(|item| {
                let visible = match item {
                    SidebarNavigation::Connector(provider) => !self
                        .workspace
                        .sidebar_hidden_connectors
                        .contains(provider.id()),
                    SidebarNavigation::Notes => self.workspace.sidebar_notes_visible,
                    SidebarNavigation::Automations => self.workspace.sidebar_automations_visible,
                    SidebarNavigation::Todos => self.workspace.sidebar_todos_visible,
                };
                MenuAction::new(format!("sidebar-{}", item.id()), item.label())
                    .icon(item.icon())
                    .toggle(visible)
                    .reorderable()
                    .disabled(item == SidebarNavigation::Notes && !self.workspace.preferences.notes)
                    .into()
            })
            .collect();
        items.extend([
            MenuItem::Separator,
            MenuAction::new("sidebar-bots", "Bots")
                .icon("ghost")
                .toggle(self.workspace.preferences.bots)
                .into(),
            MenuAction::new("sidebar-activity", "Activity")
                .icon("response-activity")
                .toggle(
                    self.workspace.sidebar_activity_visible
                        && self.workspace.preferences.working_agents,
                )
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
                    .rounded_md()
                    .relative()
                    .child(self.liquid.row("search", self.app_search.is_some()))
                    .text_color(content(
                        if self.app_search.is_some() || self.sidebar_hover == Some("Search") {
                            1.
                        } else {
                            0.5
                        },
                    ))
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|s| s.text_color(content(1.)))
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
                    .child(div().flex_1().min_w_0().h(px(20.)).child("Search")),
            )
            .children(
                self.visible_sidebar_navigation(cx)
                    .into_iter()
                    .map(|item| (item.label(), item.icon()))
                    .map(|(label, glyph)| {
                        let selected = self.app_view_label() == Some(label);
                        let connector = Connector::ALL
                            .into_iter()
                            .find(|provider| provider.label() == label);
                        let unread =
                            connector.is_some_and(|provider| self.connector_unread(provider, cx));
                        div()
                            .id(label)
                            .debug_selector(move || format!("sidebar-navigation-{label}"))
                            .role(Role::Button)
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
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(if selected || self.sidebar_hover == Some(label) {
                                1.
                            } else {
                                0.5
                            }))
                            .relative()
                            .child(self.liquid.row(label, selected))
                            .hover(|s| s.text_color(content(1.)))
                            .child(icon(glyph, 16.).text_color(content(if selected {
                                0.7
                            } else {
                                0.35
                            })))
                            .child(div().flex_1().child(label))
                            .when(unread, |row| {
                                row.child(div().size(px(6.)).rounded_full().bg(accent_color()))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| match label {
                                "GitHub" => this.open_connector(Connector::Github, window, cx),
                                "GitLab" => this.open_connector(Connector::Gitlab, window, cx),
                                "Jira" => this.open_connector(Connector::Jira, window, cx),
                                "Notes" => this.open_notes(window, cx),
                                "Automations" => this.open_automations(window, cx),
                                "To-dos" => this.open_todos(window, cx),
                                _ => (),
                            }))
                            .on_hover(cx.listener(move |this, hovered, _, cx| {
                                if *hovered {
                                    this.sidebar_hover = Some(label);
                                } else if this.sidebar_hover == Some(label) {
                                    this.sidebar_hover = None;
                                }
                                this.sidebar_frame.update(cx, |_, cx| cx.notify());
                                this.project_rail_frame.update(cx, |_, cx| cx.notify());
                            }))
                    }),
            )
            .into_any()
    }
    pub(super) fn sidebar_filter_button(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        icon_button("filter-sessions", "Filter sessions", "filter")
            .debug_selector(|| "sidebar-filter-button".into())
            .mr(px(-5.))
            .when(
                self.workspace
                    .session_filters
                    .active(self.workspace.preferences.show_archived)
                    || self.menu_open(&MenuTarget::SessionFilter),
                |d| d.bg(content(0.1)),
            )
            .relative()
            .child(self.filter_anchor.probe())
            .on_click(cx.listener(|this, _, window, cx| {
                if this.menu_open(&MenuTarget::SessionFilter) {
                    this.close_menu(cx);
                } else {
                    this.open_menu(
                        MenuTarget::SessionFilter,
                        Anchor::Slot(this.filter_anchor.clone()),
                        window,
                        cx,
                    );
                }
            }))
    }
}

#[cfg(test)]
mod tests;
