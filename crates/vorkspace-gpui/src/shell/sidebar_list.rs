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
                SidebarNavigation::Terminal => self.workspace.sidebar_terminal_visible,
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
                    SidebarNavigation::Terminal => self.workspace.sidebar_terminal_visible,
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
                "sidebar-terminal" => &mut self.workspace.sidebar_terminal_visible,
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
                    .mb_1()
                    .px_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.08))
                    // RailSearch's Tailwind shadow-sm, including
                    // its tighter second layer below the border.
                    // The vendored shader keeps it outside the
                    // box, as CSS does.
                    .shadow(vec![
                        BoxShadow::new(px(0.), px(1.), rgba(0x0000001a).into()).blur_radius(px(3.)),
                        BoxShadow::new(px(0.), px(1.), rgba(0x0000001a).into())
                            .blur_radius(px(2.))
                            .spread_radius(px(-1.)),
                    ])
                    .text_color(content(if self.sidebar_hover == Some("Search") {
                        1.
                    } else {
                        0.5
                    }))
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
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
                    .child(div().flex_1().min_w_0().h(px(20.)).child("Search"))
                    .child(
                        // The shortcut span sits outside the label's `font-medium`.
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(content(0.4))
                            .child("⌘K"),
                    ),
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
                            .id(if label == "Terminal" {
                                "terminal"
                            } else {
                                label
                            })
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
                            .when(selected, |d| d.bg(content(0.1)))
                            .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
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
                                "Terminal" => this.open_terminal_tab(window, cx),
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
mod tests {
    use super::super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn search_field_fills_the_original_project_rail(cx: &mut TestAppContext) {
        let (window, _, _) = super::super::tests::window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let sidebar = visual.debug_bounds("classic-project-rail").unwrap();
        let search = visual.debug_bounds("sidebar-search-field").unwrap();
        assert_eq!(search.left() - sidebar.left(), px(8.));
        assert_eq!(sidebar.right() - search.right(), px(9.));
    }

    #[gpui::test]
    fn sidebar_retired_density_and_activity_preferences_are_inert(cx: &mut TestAppContext) {
        let (window, a, b) = super::super::tests::window(cx);
        let card = window
            .update(cx, |shell, _, cx| {
                // Blank sessions are deliberately absent from navigation. Seed
                // a real history summary so this measures a materialized card.
                let mut summary = locked(&shell.store)
                    .unwrap()
                    .create("codex", "/work/vorkspace")
                    .unwrap();
                summary.has_messages = true;
                summary.title = "Saved conversation".into();
                let card = summary.id;
                shell.session_summaries.insert(card, summary);
                shell.order.insert(0, card);
                for id in [a, b] {
                    shell.sessions[&id].update(cx, |view, _| view.busy = true);
                }
                shell.unseen_finished.insert(a);
                shell.workspace.compact_session_cards = false;
                shell.workspace.sidebar_activity_visible = true;
                shell.invalidate_chrome(cx);
                card
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let selector: &'static str = Box::leak(format!("sidebar-session-{card}").into_boxed_str());
        assert_eq!(visual.debug_bounds(selector).unwrap().size.height, px(32.));
        assert!(visual.debug_bounds("sidebar-activity-card").is_none());
        window.update(visual, |shell, _, cx| {
            let loaded = shell.sessions.len();
            shell.change_sidebar_option("compact-session-cards", cx);
            shell.change_sidebar_option("sidebar-activity", cx);
            assert!(!shell.workspace.compact_session_cards);
            assert!(shell.workspace.sidebar_activity_visible);
            assert_eq!(shell.sessions.len(), loaded);
            assert!(shell.unseen_finished.contains(&a));
            for id in [a, b] { assert!(shell.sessions[&id].read(cx).responding()); }
            let items = shell.sidebar_options_menu_items(cx);
            assert!(!items.iter().any(|item| matches!(item, MenuItem::Action(a) if a.id == "compact-session-cards" || a.id == "sidebar-activity")));
        }).unwrap();
    }

    #[gpui::test]
    fn sidebar_visibility_switches_stay_open_preserve_notes_and_survive_restart(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = super::super::tests::window(cx);
        window
            .update(cx, |shell, w, cx| shell.open_notes(w, cx))
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let subscriptions = visual.debug_bounds("subscriptions").unwrap();
        let settings = visual.debug_bounds("classic-rail-settings").unwrap();
        assert!(subscriptions.bottom() <= settings.top());
        let options = visual.debug_bounds("sidebar-options").unwrap();
        visual.simulate_click(options.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                let menu = shell.menu.as_ref().unwrap().menu.read(cx);
                let bounds = menu.surface_region().get().unwrap();
                assert_eq!(bounds.left(), options.left());
                assert_eq!(bounds.size.width, px(212.));
            })
            .unwrap();
        let notes = visual.debug_bounds("menu-action-sidebar-notes").unwrap();
        let automations = visual
            .debug_bounds("menu-action-sidebar-automations")
            .unwrap();
        assert!(visual.debug_bounds("menu-action-sidebar-inbox").is_none());
        let bots = visual.debug_bounds("menu-action-sidebar-bots").unwrap();
        assert!(
            visual
                .debug_bounds("menu-action-sidebar-activity")
                .is_none()
        );
        assert!(
            visual
                .debug_bounds("menu-action-compact-session-cards")
                .is_none()
        );
        assert!(visual.debug_bounds("menu-grip-sidebar-notes").is_some());
        assert!(
            visual
                .debug_bounds("menu-grip-sidebar-automations")
                .is_some()
        );
        assert!(visual.debug_bounds("menu-grip-sidebar-bots").is_none());
        assert!(
            visual
                .debug_bounds("menu-grip-sidebar-subscriptions")
                .is_none()
        );
        assert!(notes.bottom() <= automations.top() && automations.bottom() < bots.top());
        for (action, navigation) in [
            ("menu-action-sidebar-notes", "sidebar-navigation-Notes"),
            (
                "menu-action-sidebar-automations",
                "sidebar-navigation-Automations",
            ),
            ("menu-action-sidebar-bots", "new-bot"),
            ("menu-action-sidebar-subscriptions", "subscriptions"),
        ] {
            assert!(visual.debug_bounds(navigation).is_some());
            let row = visual.debug_bounds(action).unwrap();
            visual.simulate_click(row.center(), Default::default());
            visual.run_until_parked();
            assert!(visual.debug_bounds(navigation).is_none());
            assert!(
                visual
                    .debug_bounds("menu-action-sidebar-subscriptions")
                    .is_some()
            );
        }
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.active_app_view, Some(app_views::AppView::Notes));
                assert!(shell.notes.is_some());
                assert!(
                    shell.workspace.preferences.notes,
                    "hiding the shortcut leaves Notes enabled"
                );
                let menu = shell.menu.as_ref().unwrap().menu.read(cx);
                for id in [
                    "sidebar-notes",
                    "sidebar-automations",
                    "sidebar-bots",
                    "sidebar-subscriptions",
                ] {
                    assert_eq!(menu.toggle_state(id), Some(false));
                }
                assert!(!cx.global::<crate::settings::Preferences>().bots);
                let saved = shell.latest_state.lock().unwrap().clone();
                let restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
                assert!(!restored.sidebar_notes_visible);
                assert!(!restored.sidebar_automations_visible);
                assert!(restored.sidebar_inbox_visible);
                assert!(!restored.sidebar_subscriptions_visible);
                assert!(!restored.preferences.bots);
            })
            .unwrap();
        for (action, navigation) in [
            ("menu-action-sidebar-notes", "sidebar-navigation-Notes"),
            (
                "menu-action-sidebar-automations",
                "sidebar-navigation-Automations",
            ),
            ("menu-action-sidebar-bots", "new-bot"),
            ("menu-action-sidebar-subscriptions", "subscriptions"),
        ] {
            let row = visual.debug_bounds(action).unwrap();
            visual.simulate_click(row.center(), Default::default());
            visual.run_until_parked();
            assert!(visual.debug_bounds(navigation).is_some());
        }
        visual.simulate_click(options.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, w, cx| {
                shell.open_settings(crate::settings::Section::Bots, w, cx)
            })
            .unwrap();
        visual.run_until_parked();
        let toggle = visual.debug_bounds("settings-toggle-bots-enabled").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, w, cx| {
                assert!(!shell.workspace.preferences.bots);
                shell.close_settings(w, cx);
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("new-bot").is_none());
        let options = visual.debug_bounds("sidebar-options").unwrap();
        visual.simulate_click(options.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                let menu = shell.menu.as_ref().unwrap().menu.read(cx);
                assert_eq!(menu.toggle_state("sidebar-bots"), Some(false));
            })
            .unwrap();
    }
    #[gpui::test]
    fn sidebar_grip_drag_reorders_without_toggling_and_preserves_connector_tabs(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::popover::bind);
        let (window, _, _) = super::super::tests::window(cx);
        let page = window
            .update(cx, |shell, w, cx| {
                shell.ensure_background_inbox(w, cx);
                shell
                    .inbox
                    .as_ref()
                    .unwrap()
                    .update(cx, |owner, _| owner.suspend_refresh_for_test());
                shell.open_connector(Connector::Github, w, cx);
                shell.connector_pages[&Connector::Github].clone()
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let options = visual.debug_bounds("sidebar-options").unwrap();
        visual.simulate_click(options.center(), Default::default());
        visual.run_until_parked();
        for id in ["github", "gitlab", "jira", "notes", "automations"] {
            let selector: &'static str =
                Box::leak(format!("menu-grip-sidebar-{id}").into_boxed_str());
            assert!(visual.debug_bounds(selector).is_some());
        }
        let grip = visual
            .debug_bounds("menu-grip-sidebar-automations")
            .unwrap();
        // A grip click is not a visibility action.
        visual.simulate_click(grip.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, w, cx| {
                assert!(shell.workspace.sidebar_automations_visible);
                assert!(
                    shell
                        .menu
                        .as_ref()
                        .unwrap()
                        .menu
                        .focus_handle(cx)
                        .is_focused(w)
                );
            })
            .unwrap();
        let end = visual
            .debug_bounds("menu-action-sidebar-github")
            .unwrap()
            .origin
            + gpui::point(px(70.), px(4.));
        visual.simulate_mouse_down(grip.center(), MouseButton::Left, Default::default());
        visual.simulate_mouse_move(
            grip.center() + gpui::point(px(0.), px(-8.)),
            MouseButton::Left,
            Default::default(),
        );
        visual.run_until_parked();
        visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, w, cx| {
                assert_eq!(
                    shell.workspace.sidebar_navigation_order,
                    [
                        "automations",
                        "github",
                        "gitlab",
                        "jira",
                        "notes",
                        "terminal"
                    ]
                );
                assert!(shell.workspace.sidebar_automations_visible);
                assert!(shell.workspace.sidebar_hidden_connectors.is_empty());
                let menu = shell.menu.as_ref().unwrap().menu.clone();
                assert!(menu.focus_handle(cx).is_focused(w));
            })
            .unwrap();
        // Escape cancels an in-flight drag, without committing or closing the menu.
        let grip = visual
            .debug_bounds("menu-grip-sidebar-notes")
            .unwrap()
            .center();
        visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(
            grip + gpui::point(px(0.), px(-10.)),
            MouseButton::Left,
            Default::default(),
        );
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert!(shell.menu.is_some());
                assert!(!cx.has_active_drag());
                assert_eq!(shell.workspace.sidebar_navigation_order[0], "automations");
            })
            .unwrap();
        let toggle = visual.debug_bounds("menu-action-sidebar-github").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("sidebar-navigation-GitHub").is_none());
        assert!(visual.debug_bounds("app-title-tab-GitHub").is_some());
        window
            .update(visual, |shell, _, cx| {
                assert!(shell.enabled_connectors(cx).contains(&Connector::Github));
                assert_eq!(shell.connector_pages[&Connector::Github], page);
                let saved = shell.latest_state.lock().unwrap().clone();
                let restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
                assert_eq!(
                    restored.sidebar_navigation_order,
                    shell.workspace.sidebar_navigation_order
                );
                assert!(restored.sidebar_hidden_connectors.contains("github"));
            })
            .unwrap();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        // The same saved projection drives both layouts; Search remains fixed.
        for expanded in [true, false, true] {
            window
                .update(visual, |shell, _, cx| {
                    if shell.workspace.project_rail_open != expanded {
                        shell.toggle_project_rail(cx);
                    }
                    assert_eq!(shell.workspace.project_rail_open, expanded);
                })
                .unwrap();
            visual.run_until_parked();
            let names = [
                "sidebar-navigation-Automations",
                "sidebar-navigation-GitLab",
                "sidebar-navigation-Jira",
                "sidebar-navigation-Notes",
            ];
            let bounds: Vec<_> = names
                .into_iter()
                .map(|id| visual.debug_bounds(id).unwrap())
                .collect();
            assert!(
                bounds
                    .windows(2)
                    .all(|pair| pair[0].bottom() <= pair[1].top())
            );
            let search = visual
                .debug_bounds(if expanded {
                    "sidebar-search-field"
                } else {
                    "sidebar-navigation-Search"
                })
                .unwrap();
            assert!(search.bottom() <= bounds[0].top());
        }
        // Disable and re-enable through the existing owner; neither operation
        // discards custom position or the independently hidden shortcut.
        for enabled in [false, true] {
            window
                .update(visual, |shell, _, cx| {
                    shell.inbox.as_ref().unwrap().update(cx, |owner, cx| {
                        owner.set_connector_enabled("gitlab", enabled, cx).unwrap()
                    });
                    shell.invalidate_chrome(cx);
                    let items = shell.sidebar_options_menu_items(cx);
                    assert_eq!(
                        items.iter().any(
                            |item| matches!(item, MenuItem::Action(a) if a.id == "sidebar-gitlab")
                        ),
                        enabled
                    );
                    assert_eq!(
                        shell.workspace.sidebar_navigation_order,
                        [
                            "automations",
                            "github",
                            "gitlab",
                            "jira",
                            "notes",
                            "terminal"
                        ]
                    );
                    assert!(shell.workspace.sidebar_hidden_connectors.contains("github"));
                })
                .unwrap();
        }
    }

    #[test]
    #[ignore = "Requires Metal and VOKKY_SIDEBAR_CAPTURE_DIR"]
    fn sidebar_settings_native_capture() {
        let output =
            std::path::PathBuf::from(std::env::var_os("VOKKY_SIDEBAR_CAPTURE_DIR").unwrap());
        assert!(output.is_absolute() && output.is_dir());
        let store = Arc::new(Mutex::new(
            vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let session = locked(&store)
            .unwrap()
            .create("codex", "/work/sidebar-fixture")
            .unwrap();
        let mut state = Workspace::default();
        state.open(session.id);
        state.preferences.tab_animations = false;
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
        crate::clock::use_test_clock(&cx);
        let window = cx
            .open_window(gpui::size(px(1280.), px(800.)), |_, cx| {
                cx.new(|cx| {
                    Shell::new(
                        store,
                        vec![session],
                        "/work/sidebar-fixture".into(),
                        Some(saved),
                        cx,
                    )
                })
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(&mut cx, |shell, w, cx| {
                shell.open_menu(
                    MenuTarget::SidebarOptions,
                    Anchor::Slot(shell.sidebar_options_anchor.clone()),
                    w,
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        crate::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join("sidebar-options.png"))
            .unwrap();
        window
            .update(&mut cx, |shell, _, cx| {
                shell.reorder_sidebar_navigation(
                    &crate::popover::MenuReordered {
                        source: "sidebar-automations".into(),
                        target: "sidebar-github".into(),
                        after: false,
                    },
                    cx,
                );
                shell.change_sidebar_option("sidebar-gitlab", cx);
                shell.close_menu(cx);
            })
            .unwrap();
        for (name, expanded) in [("expanded", true), ("collapsed", false)] {
            window
                .update(&mut cx, |shell, _, cx| {
                    if shell.workspace.project_rail_open != expanded {
                        shell.toggle_project_rail(cx);
                    }
                })
                .unwrap();
            cx.run_until_parked();
            crate::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("sidebar-reordered-{name}.png")))
                .unwrap();
        }
        window
            .update(&mut cx, |shell, w, cx| {
                shell.open_settings(crate::settings::Section::Connections, w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        crate::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join("settings-app-group.png"))
            .unwrap();
    }

    #[gpui::test]
    fn scroll_and_hover_redraws_reuse_the_sidebar_index(cx: &mut TestAppContext) {
        let (window, _, _) = super::super::tests::window(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.refresh_sidebar_structure(cx);
                let rebuilds = shell.sidebar_list.structural_rebuilds;
                for _ in 0..100 {
                    // This is the path used by scroll/hover redraws: visible
                    // cards refresh, while membership/order stays cached.
                    shell.refresh_sidebar_structure(cx);
                }
                assert_eq!(shell.sidebar_list.structural_rebuilds, rebuilds);
                shell.sidebar_list.invalidate_structure();
                shell.refresh_sidebar_structure(cx);
                assert_eq!(shell.sidebar_list.structural_rebuilds, rebuilds + 1);
                // Time expiry is a content refresh, not navigation: retain
                // the user's position instead of snapping back to the active row.
                shell.sidebar_list.state.scroll_to(gpui::ListOffset {
                    item_ix: 1,
                    offset_in_item: px(5.),
                });
                let offset = shell.sidebar_list.state.logical_scroll_top();
                shell.sidebar_list.indexed_minute = 0;
                shell.refresh_sidebar_structure(cx);
                let refreshed = shell.sidebar_list.state.logical_scroll_top();
                assert_eq!(refreshed.item_ix, offset.item_ix);
                assert_eq!(refreshed.offset_in_item, offset.offset_in_item);
                assert_eq!(shell.sidebar_list.structural_rebuilds, rebuilds + 2);
                // Scope changes are also detected without a notification hook.
                shell.query.push_str("unmatched-session-query");
                shell.refresh_sidebar_structure(cx);
                assert_eq!(shell.sidebar_list.structural_rebuilds, rebuilds + 3);
                assert!(shell.sidebar_list.rows.contains(&super::Row::Empty));
            })
            .unwrap();
    }

    #[gpui::test]
    fn large_project_only_lays_out_nearby_session_cards(cx: &mut TestAppContext) {
        let (window, _, _) = super::super::tests::window(cx);
        let started = std::time::Instant::now();
        let (first, last) = window
            .update(cx, |shell, window, cx| {
                let mut summaries = Vec::new();
                {
                    let store = locked(&shell.store).unwrap();
                    for i in 0..1000 {
                        let mut summary = store.create("codex", "/work/many-sessions").unwrap();
                        summary.title =
                            format!("Session {i}: {}", "A long conversation title 🙂 ".repeat(4));
                        summary.has_messages = true;
                        summaries.push(summary);
                    }
                }
                let first = summaries.last().unwrap().id;
                let last = summaries[0].id;
                for summary in summaries {
                    shell.order.push(summary.id);
                    shell.session_summaries.insert(summary.id, summary);
                }
                shell.workspace.open(first);
                shell.select_project("/work/many-sessions".into(), window, cx);
                (first, last)
            })
            .unwrap();
        cx.refresh().unwrap();
        window
            .update(cx, |shell, _, cx| {
                let laid_out = shell
                    .session_row_bounds
                    .values()
                    .filter(|slot| slot.get().is_some())
                    .count();
                eprintln!(
                    "1,000-session project: {laid_out} laid-out cards, {:?} setup + first frame",
                    started.elapsed()
                );
                assert!(laid_out > 0);
                assert!(
                    shell.sessions.len() < 10,
                    "scrolling the sidebar must not construct off-screen conversations"
                );
                assert!(
                    laid_out < 80,
                    "off-screen session cards should not be laid out: {laid_out}"
                );
                assert!(shell.session_row_bounds[&first].get().is_some());
                assert!(shell.session_row_bounds[&last].get().is_none());
                shell.sidebar_list.state.scroll_to_end();
                shell.sidebar_frame.update(cx, |_, cx| cx.notify());
            })
            .unwrap();
        cx.refresh().unwrap();
        window
            .update(cx, |shell, _, cx| {
                assert!(
                    shell.session_row_bounds[&first].get().is_none(),
                    "scrolled-away cards cannot remain hit targets"
                );
                assert!(
                    shell.session_row_bounds[&last].get().is_some(),
                    "the oldest conversation is reachable"
                );
                let members = shell.sidebar_session_ids(cx);
                shell.groups.insert(
                    "/work/many-sessions".into(),
                    vorkspace_core::session_groups::Groups {
                        folders: vec![vorkspace_core::session_groups::Folder {
                            id: "large-folder".into(),
                            name: "All history".into(),
                            session_ids: members,
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                );
                shell.notify_sidebar(cx);
            })
            .unwrap();
        cx.refresh().unwrap();
        window
            .update(cx, |shell, _, _| {
                let laid_out = shell
                    .session_row_bounds
                    .values()
                    .filter(|slot| slot.get().is_some())
                    .count();
                assert!(
                    laid_out > 0 && laid_out < 80,
                    "expanded folders must also be virtualized: {laid_out}"
                );
                shell.sidebar_list.state.scroll_to_end();
            })
            .unwrap();
        cx.refresh().unwrap();
        let offset = window
            .update(cx, |shell, window, cx| {
                // Resolve scroll_to_end's end sentinel before checking selection.
                let offset = shell.sidebar_list.state.logical_scroll_top();
                shell.open(first - 1, window, cx);
                offset
            })
            .unwrap();
        cx.refresh().unwrap();
        window
            .update(cx, |shell, _, _| {
                // Like the Tauri sidebar, selecting a session (e.g. from a
                // title tab) leaves the user's scroll position alone.
                let after = shell.sidebar_list.state.logical_scroll_top();
                assert_eq!(
                    (after.item_ix, after.offset_in_item),
                    (offset.item_ix, offset.offset_in_item)
                );
                assert!(shell.session_row_bounds[&(first - 1)].get().is_none());
            })
            .unwrap();
    }
}
