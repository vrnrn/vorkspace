//! The original project rail is independent of the project's session/file pane.
use super::*;
use serde_json::Value;

const COMPACT_WIDTH: f32 = 48.;

// The divider reserves one column; center footer hit areas in the remaining
// content width, without the expanded sidebar's horizontal padding.
fn compact_footer(
    resources: Entity<crate::resource_pressure::ResourceMonitor>,
    usage: Option<Entity<crate::usage_footer::UsageFooter>>,
    settings: impl IntoElement,
) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .py_2()
        .child(resources)
        .children(usage)
        .child(settings)
}

fn compact_settings_button() -> gpui::Stateful<gpui::Div> {
    icon_button("classic-rail-settings", "Settings", "settings")
        .debug_selector(|| "classic-rail-settings".into())
        .focusable()
        .tab_stop(true)
        .size(px(32.))
}

fn metadata(workspace: &Workspace, key: &str) -> Value {
    workspace
        .desktop_preferences_metadata
        .as_ref()
        .and_then(|values| values.get(key))
        .and_then(Value::as_str)
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or(Value::Null)
}

impl Shell {
    pub(super) fn classic_rail_width(&self) -> f32 {
        if self.workspace.project_rail_open || self.settings.is_some() {
            self.workspace.project_rail_width
        } else if self.workspace.preferences.collapsed_project_rail == "hidden" {
            0.
        } else {
            COMPACT_WIDTH
        }
    }

    pub(super) fn toggle_project_rail(&mut self, cx: &mut Context<Self>) {
        self.workspace.project_rail_open = !self.workspace.project_rail_open;
        self.project_rail_drag = None;
        self.close_menu(cx);
        self.fit_sidebar(cx);
        self.invalidate_chrome(cx);
        self.persist(cx);
        cx.notify();
    }

    /// Channels occupy the same main surface as Bots, with the project rail
    /// still available. They do not invent a session or change its project.
    pub(crate) fn open_classic_channels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_app_views(cx);
        self.leave_app_search(cx);
        self.close_preview(cx);
        self.navigation_revision += 1;
        self.invalidate_chrome(cx);
        self.focus_workspace(window, cx);
        cx.notify();
    }

    pub(super) fn classic_project_rail(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = self.classic_rail_width();
        // Early-return layouts must stop the card's presentation clock too.
        if width <= 0. || self.settings.is_some() {
            self.live_agents.update(cx, |activity, _| activity.hide());
        }
        if width <= 0. {
            return div()
                .id("classic-project-rail")
                .debug_selector(|| "classic-project-rail".into())
                .w_0()
                .into_any_element();
        }
        let mut rail = div()
            .id("classic-project-rail")
            .debug_selector(|| "classic-project-rail".into())
            .role(Role::Navigation)
            .aria_label("Projects")
            .relative()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            // Retain the separator's column in content layout, without painting
            // a full-height border through the native titlebar controls.
            .pr_px()
            // The titlebar belongs to native controls and window navigation.
            // A narrow rail's separator must never cut through that cluster.
            .child(
                div()
                    .debug_selector(|| "classic-project-rail-divider".into())
                    .absolute()
                    .right_0()
                    .top(px(TITLE_BAR_HEIGHT))
                    .bottom_0()
                    .w_px()
                    .bg(content(0.08)),
            )
            .child(div().h(px(TITLE_BAR_HEIGHT)).flex_shrink_0())
            .child(crate::bots::avatar::sidebar_cursor_tracker());

        self.resources.update(cx, |resources, cx| {
            resources.set_compact(
                !self.workspace.project_rail_open && self.settings.is_none(),
                cx,
            )
        });

        if let Some(settings) = &self.settings {
            let section = settings.read(cx).section;
            let select = cx.weak_entity();
            let back = select.clone();
            return rail
                .child(crate::settings::nav(
                    section,
                    move |section, _, cx| {
                        let _ = select.update(cx, |this, cx| this.select_settings(section, cx));
                    },
                    move |window, cx| {
                        let _ = back.update(cx, |this, cx| this.close_settings(window, cx));
                    },
                ))
                .child(div().mx_2().child(self.resources.clone()))
                .into_any_element();
        }

        self.usage_footer.update(cx, |usage, cx| {
            usage.set_compact(!self.workspace.project_rail_open, cx)
        });

        if !self.workspace.project_rail_open {
            return self.compact_project_rail(rail, cx);
        }

        let projects = self.workspace.projects.clone();
        let busy: HashSet<_> = self
            .sessions
            .values()
            .filter_map(|view| {
                let session = view.read(cx);
                session.responding().then(|| session.summary.cwd.clone())
            })
            .collect();
        let groups = metadata(&self.workspace, "vorkspace.projectGroups");
        let assignments = metadata(&self.workspace, "vorkspace.projectGroupAssignments");
        let pinned = metadata(&self.workspace, "vorkspace.projectRailPinned");
        let pinned: HashSet<_> = pinned
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(vorkspace_core::files::path_key)
            .collect();
        let pinned_projects: Vec<_> = projects
            .iter()
            .enumerate()
            .filter(|(_, path)| pinned.contains(&vorkspace_core::files::path_key(path)))
            .map(|(ix, path)| self.classic_project_row(ix, path.clone(), busy.contains(path), cx))
            .collect();
        let mut group_rows = Vec::new();
        let mut grouped_paths = HashSet::new();
        if let Some(groups) = groups.as_array() {
            for group in groups {
                let (Some(id), Some(name)) = (group["id"].as_str(), group["name"].as_str()) else {
                    continue;
                };
                let collapsed = self
                    .workspace
                    .project_group_collapsed
                    .get(id)
                    .copied()
                    .unwrap_or(group["collapsed"] == true);
                let members: Vec<_> = projects
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| {
                        let key = vorkspace_core::files::path_key(path);
                        !pinned.contains(&key) && assignments[key] == id
                    })
                    .map(|(ix, path)| {
                        grouped_paths.insert(path.clone());
                        (ix, path.clone())
                    })
                    .collect();
                let id = id.to_owned();
                let toggle = id.clone();
                let row = div()
                    .id(SharedString::from(format!("classic-project-group-{id}")))
                    .debug_selector(move || format!("classic-project-group-{id}"))
                    .role(Role::Button)
                    .aria_label(format!("{name}, {} projects", members.len()))
                    .aria_expanded(!collapsed)
                    .focusable()
                    .tab_stop(true)
                    .h(px(32.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .text_size(px(14.))
                    .text_color(content(0.65))
                    .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                    .focus(|s| s.bg(content(0.08)))
                    .child(icon(
                        if collapsed {
                            "chevron-right"
                        } else {
                            "chevron-down"
                        },
                        14.,
                    ))
                    .child(div().min_w_0().truncate().child(name.to_owned()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_classic_project_group(&toggle, cx);
                    }));
                let mut section = div()
                    .flex_shrink_0()
                    .mx_2()
                    .mb_1p5()
                    .rounded_md()
                    .when(!collapsed, |d| d.bg(content(0.05)))
                    .child(row);
                if !collapsed {
                    section = section.child(div().p_1().flex().flex_col().gap_px().children(
                        members.into_iter().map(|(ix, path)| {
                            self.classic_project_row(ix, path.clone(), busy.contains(&path), cx)
                        }),
                    ));
                }
                group_rows.push(section.into_any_element());
            }
        }

        let bots = self.bots.clone().map(|bots| {
            bots.update(cx, |bots, cx| {
                div()
                    .child(bots.rail(None, cx))
                    .child(bots.classic_channels_rail(cx))
                    .into_any_element()
            })
        });
        let navigation = self.sidebar_navigation(cx);
        rail = rail
            .child(navigation)
            .child(
                div()
                    .id("classic-project-rail-scroll")
                    .debug_selector(|| "classic-project-rail-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pb_2()
                    .children(bots)
                    .when(!pinned_projects.is_empty(), |d| {
                        d.child(section_label("Pinned").mx_2()).child(
                            div()
                                .px_2()
                                .pb_3()
                                .flex()
                                .flex_col()
                                .gap_px()
                                .children(pinned_projects),
                        )
                    })
                    .when(!group_rows.is_empty(), |d| {
                        d.child(section_label("Groups").mx_2()).children(group_rows)
                    })
                    .child(
                        section_label("Projects").mx_2().child(
                            icon_button("classic-open-project", "Open project", "plus")
                                .debug_selector(|| "classic-open-project".into())
                                .focusable()
                                .tab_stop(true)
                                .size(px(20.))
                                .on_click(cx.listener(|this, _, _, cx| this.open_project(cx))),
                        ),
                    )
                    .child(
                        div().px_2().flex().flex_col().gap_px().children(
                            projects
                                .into_iter()
                                .enumerate()
                                .filter(|(_, path)| {
                                    !grouped_paths.contains(path)
                                        && !pinned.contains(&vorkspace_core::files::path_key(path))
                                })
                                .map(|(ix, path)| {
                                    self.classic_project_row(
                                        ix,
                                        path.clone(),
                                        busy.contains(&path),
                                        cx,
                                    )
                                }),
                        ),
                    ),
            )
            .when(
                self.notification_state.installed.read(cx).is_visible(),
                |d| {
                    d.child(
                        div()
                            .mx_2()
                            .mb_2()
                            .child(self.notification_state.installed.clone()),
                    )
                },
            )
            .child(div().mx_2().child(self.resources.clone()))
            .children(self.live_agents_preview(false, cx))
            .when(self.workspace.sidebar_subscriptions_visible, |d| {
                d.child(div().px_2().child(self.usage_footer.clone()))
            })
            .child(self.classic_rail_footer(cx))
            .child(
                div()
                    .id("classic-project-rail-resize")
                    .debug_selector(|| "classic-project-rail-resize".into())
                    .absolute()
                    .right(px(-2.))
                    .top(px(TITLE_BAR_HEIGHT))
                    .bottom_0()
                    .w(px(4.))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.project_rail_drag =
                                Some((event.position.x, this.workspace.project_rail_width));
                            cx.stop_propagation();
                        }),
                    ),
            );
        rail.into_any_element()
    }

    fn toggle_classic_project_group(&mut self, id: &str, cx: &mut Context<Self>) {
        let groups = metadata(&self.workspace, "vorkspace.projectGroups");
        let Some(group) = groups
            .as_array()
            .and_then(|groups| groups.iter().find(|group| group["id"] == id))
        else {
            return;
        };
        let collapsed = self
            .workspace
            .project_group_collapsed
            .get(id)
            .copied()
            .unwrap_or(group["collapsed"] == true);
        self.workspace
            .project_group_collapsed
            .insert(id.to_owned(), !collapsed);
        self.invalidate_chrome(cx);
        self.persist(cx);
        cx.notify();
    }

    fn classic_project_row(
        &self,
        ix: usize,
        path: String,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.workspace.project == path
            && self.app_search.is_none()
            && !self.app_view_open()
            && !self.bot_active(cx);
        let appearance = self.appearance(&path);
        let menu = path.clone();
        let options = path.clone();
        div()
            .id(("classic-project", ix))
            .debug_selector(move || format!("classic-project-{ix}"))
            .role(Role::Button)
            .aria_label(format!(
                "Project {}{}",
                self.project_label(&path),
                if busy { ", working" } else { "" }
            ))
            .aria_description(path.clone())
            .aria_selected(selected)
            .focusable()
            .tab_stop(true)
            .group("classic-project-row")
            .relative()
            .overflow_hidden()
            .h(px(32.))
            .min_w_0()
            .flex_shrink_0()
            .px_2()
            .rounded_md()
            .flex()
            .items_center()
            .gap_2()
            .opacity(if selected { 1. } else { 0.65 })
            .when(selected, |d| {
                d.child(crate::bots::avatar::sidebar_selection_pill(
                    appearance.seed(&path),
                    appearance.grid(),
                    appearance.textured,
                    appearance.color(&path),
                    appearance.fg_color(&path),
                ))
            })
            .hover(|s| s.bg(content(0.08)).opacity(1.))
            .focus(|s| s.bg(content(0.1)).opacity(1.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .debug_selector(move || format!("classic-project-label-{ix}"))
                    .child(self.project_label(&path)),
            )
            .when(busy, |d| {
                d.child(div().size(px(5.)).rounded_full().bg(accent_color()))
            })
            .child(
                div()
                    .size(px(20.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(1.)
                    .group_hover("classic-project-row", |s| s.invisible())
                    .child(self.project_mark(&path, 16.)),
            )
            .child(
                icon_button(
                    "classic-project-options",
                    "Project options",
                    "more-horizontal",
                )
                .size(px(20.))
                .focusable()
                .tab_stop(true)
                .absolute()
                .right_2()
                .top(px(6.))
                .invisible()
                .focus(|s| s.visible())
                .group_hover("classic-project-row", |s| s.visible())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(
                    move |this, event: &gpui::ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_menu(
                            MenuTarget::Project {
                                path: options.clone(),
                                rail: true,
                            },
                            Anchor::Point(event.position()),
                            window,
                            cx,
                        );
                    },
                )),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_menu(
                        MenuTarget::Project {
                            path: menu.clone(),
                            rail: true,
                        },
                        Anchor::Point(event.position),
                        window,
                        cx,
                    );
                }),
            )
            .on_click(
                cx.listener(move |this, _, window, cx| {
                    this.select_project(path.clone(), window, cx)
                }),
            )
            .into_any_element()
    }

    fn classic_rail_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(40.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("classic-rail-settings")
                    .debug_selector(|| "classic-rail-settings".into())
                    .role(Role::Button)
                    .aria_label("Settings")
                    .focusable()
                    .tab_stop(true)
                    .flex_1()
                    .h(px(32.))
                    .px_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(14.))
                    .text_color(content(0.5))
                    .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                    .focus(|s| s.bg(content(0.08)))
                    .child(icon("settings", 16.))
                    .child(div().flex_1().child("Settings"))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child("⌘,"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings(crate::settings::Section::General, window, cx)
                    })),
            )
            .child(
                icon_button(
                    "classic-sidebar-options",
                    "Sidebar options",
                    "more-horizontal",
                )
                .debug_selector(|| "sidebar-options".into())
                .child(self.sidebar_options_anchor.probe())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_menu(
                        MenuTarget::SidebarOptions,
                        Anchor::Slot(this.sidebar_options_anchor.clone()),
                        window,
                        cx,
                    );
                })),
            )
            .into_any_element()
    }

    fn compact_project_rail(
        &mut self,
        rail: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::workspace::SidebarTab;

        let workspace_active =
            !self.app_view_open() && self.app_search.is_none() && !self.bot_active(cx);
        let tab = self.workspace.sidebar_tab();
        let tabs = SidebarTab::normalize(&self.workspace.sidebar_tab_order);
        let pinned_bots: Vec<_> = if self.workspace.preferences.rail_bots_pinned {
            self.bots
                .as_ref()
                .filter(|_| self.workspace.preferences.bots)
                .map(|manager| {
                    let manager = manager.read(cx);
                    manager
                        .roster
                        .bots
                        .iter()
                        .map(|bot| {
                            let id = bot.id.clone();
                            let selected = manager.active.as_ref() == Some(&id);
                            let selector = format!("classic-compact-bot-{id}");
                            let manager = self.bots.clone().unwrap();
                            div()
                                .id(SharedString::from(selector.clone()))
                                .debug_selector(move || selector.clone())
                                .role(Role::Button)
                                .aria_label(format!("Bot {}", bot.label()))
                                .aria_selected(selected)
                                .focusable()
                                .tab_stop(true)
                                .size(px(32.))
                                .rounded_md()
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(selected, |d| d.bg(content(0.1)))
                                .hover(|s| s.bg(content(0.08)))
                                .child(crate::bots::avatar::bot_avatar(bot, "idle", 18., true))
                                .on_click(move |_, window, cx| {
                                    manager.update(cx, |manager, cx| manager.open(&id, window, cx));
                                })
                                .into_any_element()
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let unread = self.unseen_finished.iter().any(|id| {
            self.session_summaries
                .get(id)
                .is_some_and(|session| session.cwd == self.workspace.project)
        });
        let changes = self
            .right_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).git.read(cx).has_changes());
        rail.child(
            div()
                .id("classic-compact-actions")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_2()
                .py_1p5()
                .flex()
                .flex_col()
                .items_center()
                .gap_1p5()
                .when(!pinned_bots.is_empty(), |d| {
                    d.child(div().flex().flex_col().gap_1p5().children(pinned_bots))
                        .child(div().h_px().w(px(24.)).bg(content(0.08)))
                })
                .child(
                    div()
                        .id("classic-compact-project-picker")
                        .debug_selector(|| "classic-compact-project-picker".into())
                        .role(Role::Button)
                        .aria_label(format!(
                            "Switch project, {}",
                            self.project_label(&self.workspace.project)
                        ))
                        .aria_expanded(self.menu_open(&MenuTarget::Header))
                        .focusable()
                        .tab_stop(true)
                        .size(px(32.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|s| s.bg(content(0.08)))
                        .child(self.project_mark(&self.workspace.project, 14.))
                        .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                            this.open_menu(
                                MenuTarget::Header,
                                Anchor::Point(event.position()),
                                window,
                                cx,
                            );
                        })),
                )
                .child(
                    div()
                        .id("classic-compact-workspace-tabs")
                        .role(Role::TabList)
                        .aria_label("Workspace")
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .children(tabs.into_iter().map(|item| {
                            let current = workspace_active
                                && (tab == item
                                    || (tab == SidebarTab::Graph && item == SidebarTab::Changes));
                            let selected = current && self.workspace.sidebar_visible;
                            let unread = item == SidebarTab::Sessions && unread;
                            let dirty = item == SidebarTab::Changes && changes;
                            let dot = unread || dirty;
                            let selector =
                                format!("classic-compact-tab-{}", item.label().to_lowercase());
                            div()
                                .id(SharedString::from(selector.clone()))
                                .debug_selector(move || selector.clone())
                                .role(Role::Tab)
                                .aria_label(if unread {
                                    format!("{}, new replies", item.label())
                                } else if dirty {
                                    "Changes, uncommitted changes".to_owned()
                                } else {
                                    item.label().to_owned()
                                })
                                .aria_selected(selected)
                                .focusable()
                                .tab_stop(true)
                                .relative()
                                .size(px(32.))
                                .rounded_md()
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(selected, |d| d.bg(content(0.1)))
                                .hover(|s| s.bg(content(0.08)))
                                .child(icon(item.icon(), 18.).text_color(content(if selected {
                                    0.9
                                } else {
                                    0.5
                                })))
                                .when(dot, |d| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .right(px(3.))
                                            .top(px(3.))
                                            .size(px(5.))
                                            .rounded_full()
                                            .bg(accent_color()),
                                    )
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_classic_sidebar(item, window, cx)
                                }))
                                .on_hover(cx.listener(move |this, hovered, _, cx| {
                                    if current && !this.workspace.sidebar_visible {
                                        this.hover_peek_toggle(*hovered, cx);
                                    }
                                }))
                        })),
                )
                .children(
                    std::iter::once(("Search", "search"))
                        .chain(
                            self.visible_sidebar_navigation(cx)
                                .into_iter()
                                .map(|item| (item.label(), item.icon())),
                        )
                        .map(|(label, glyph)| {
                            let selected = if label == "Search" {
                                self.app_search.is_some()
                            } else {
                                self.app_view_label() == Some(label)
                            };
                            let connector = Connector::ALL
                                .into_iter()
                                .find(|provider| provider.label() == label);
                            let unread = connector
                                .is_some_and(|provider| self.connector_unread(provider, cx));
                            icon_button(
                                if label == "Terminal" {
                                    "terminal"
                                } else {
                                    label
                                },
                                label,
                                glyph,
                            )
                            .debug_selector(move || format!("sidebar-navigation-{label}"))
                            .aria_label(if unread {
                                format!("{label}, new items")
                            } else {
                                label.into()
                            })
                            .aria_selected(selected)
                            .focusable()
                            .tab_stop(true)
                            .size(px(32.))
                            .relative()
                            .when(selected, |d| d.bg(content(0.1)))
                            .when(unread, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .right(px(3.))
                                        .top(px(3.))
                                        .size(px(5.))
                                        .rounded_full()
                                        .bg(accent_color()),
                                )
                            })
                            .on_click(cx.listener(
                                move |this, _, window, cx| match label {
                                    "Search" => this.focus_search(window, cx),
                                    "GitHub" => this.open_connector(Connector::Github, window, cx),
                                    "GitLab" => this.open_connector(Connector::Gitlab, window, cx),
                                    "Jira" => this.open_connector(Connector::Jira, window, cx),
                                    "Notes" => this.open_notes(window, cx),
                                    "Automations" => this.open_automations(window, cx),
                                    "Terminal" => this.open_terminal_tab(window, cx),
                                    _ => (),
                                },
                            ))
                        }),
                ),
        )
        .children(self.live_agents_preview(true, cx))
        .child(compact_footer(
            self.resources.clone(),
            self.workspace
                .sidebar_subscriptions_visible
                .then(|| self.usage_footer.clone()),
            compact_settings_button().on_click(cx.listener(|this, _, window, cx| {
                this.open_settings(crate::settings::Section::General, window, cx)
            })),
        ))
        .into_any_element()
    }
}

#[cfg(test)]
#[path = "compact_footer_tests.rs"]
mod compact_footer_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use serde_json::json;
    use vorkspace_core::store::Store;

    #[gpui::test]
    fn original_rail_keeps_pins_groups_and_all_projects_through_a_saved_group_toggle(
        cx: &mut TestAppContext,
    ) {
        let root = std::env::temp_dir().join(format!(
            "classic-projects-{}",
            vorkspace_core::bot::new_id()
        ));
        let projects: Vec<_> = (0..7)
            .map(|ix| {
                let path = root.join(format!("project-{ix}"));
                std::fs::create_dir_all(&path).unwrap();
                path.to_string_lossy().into_owned()
            })
            .collect();
        let store = Arc::new(Mutex::new(
            Store::open(&root.join("native.sqlite")).unwrap(),
        ));
        let database = store.clone();
        let initial = projects[0].clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Shell::new(database, Vec::new(), initial, None, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        let source = json!({
            "vorkspace.projectGroups": "[{\"id\":\"work\",\"name\":\"Work\",\"collapsed\":true,\"customColor\":\"#9988ff\"}]",
            "vorkspace.projectGroupAssignments": json!({vorkspace_core::files::path_key(&projects[5]): "work"}).to_string(),
            "vorkspace.projectRailPinned": json!([projects[6]]).to_string(),
        });
        window
            .update(cx, |shell, _, cx| {
                shell.workspace.projects = projects.clone();
                shell.workspace.desktop_preferences_metadata = Some(source.clone());
                shell.workspace.project_group_collapsed.clear();
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1400.), px(1000.)));
        visual.run_until_parked();
        let pinned = visual.debug_bounds("classic-project-6").unwrap();
        let group = visual.debug_bounds("classic-project-group-work").unwrap();
        let ungrouped = visual.debug_bounds("classic-project-0").unwrap();
        assert!(pinned.top() < group.top());
        assert!(group.top() < ungrouped.top());
        assert!(visual.debug_bounds("classic-project-5").is_none());
        visual.simulate_click(group.center(), Default::default());
        visual.run_until_parked();
        for selector in [
            "classic-project-0",
            "classic-project-1",
            "classic-project-2",
            "classic-project-3",
            "classic-project-4",
            "classic-project-5",
            "classic-project-6",
        ] {
            assert!(visual.debug_bounds(selector).is_some());
        }
        let saved = locked(&store).unwrap().ui_state().unwrap().unwrap();
        let reopened = Workspace::restore(Some(&saved), &[], &projects[0]);
        assert_eq!(reopened.project_group_collapsed.get("work"), Some(&false));
        assert_eq!(reopened.desktop_preferences_metadata, Some(source));
        window
            .update(visual, |shell, _, cx| {
                shell.workspace = reopened;
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert!(visual.debug_bounds("classic-project-5").is_some());
    }
}
