//! The original project rail is independent of the project's session/file pane.
use super::*;
use crate::{session_mark::ProjectMark, theme::StatusColor};

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

    pub(super) fn hide_project_marks(&mut self, cx: &mut Context<Self>) {
        self.retain_project_marks(|_| false, cx);
    }

    fn retain_project_marks(
        &mut self,
        mut visible: impl FnMut(&str) -> bool,
        cx: &mut Context<Self>,
    ) {
        self.project_marks.retain(|path, mark| {
            if visible(path) {
                true
            } else {
                mark.update(cx, |mark, _| mark.hide());
                false
            }
        });
    }

    pub(super) fn classic_project_rail(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = self.classic_rail_width();
        if width <= 0. || self.settings.is_some() || !self.workspace.project_rail_open {
            self.hide_project_marks(cx);
        }
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
            // The divider, and the hovered and selected rows' pills running
            // into the pane (crate::liquid), below the native titlebar controls.
            .child({
                // Around its neighbour's rounded corners (the pane, or a left
                // sidebar); the top one not beside the icon rail, whose
                // navigation overhangs it.
                self.liquid.surface(
                    self.workspace.project_rail_open || self.settings.is_some(),
                    true,
                )
            })
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
                    &self.liquid,
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
        // Expanded, sessions hang under their projects instead of the Working card.
        self.live_agents.update(cx, |activity, _| activity.hide());

        let projects = self.workspace.projects.clone();
        let _ = self.workspace.migrate_project_rail();
        let rail_state = self
            .workspace
            .project_rail_state
            .clone()
            .unwrap_or_default();
        let groups = &rail_state.groups;
        let assignments = &rail_state.assignments;
        let pinned = &rail_state.pinned;
        let pinned_projects: Vec<_> = projects
            .iter()
            .enumerate()
            .filter(|(_, path)| pinned.contains(&vorkspace_core::files::path_key(path)))
            .map(|(ix, path)| self.classic_project_row(ix, path.clone(), cx))
            .collect();
        let mut group_rows = Vec::new();
        let mut grouped_paths = HashSet::new();
        let mut hidden_projects = HashSet::new();
        {
            for group in groups {
                let id = group.id.as_str();
                let name = group.name.as_str();
                let collapsed = self
                    .workspace
                    .project_group_collapsed
                    .get(id)
                    .copied()
                    .unwrap_or(group.collapsed);
                let members: Vec<_> = projects
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| {
                        let key = vorkspace_core::files::path_key(path);
                        !pinned.contains(&key)
                            && assignments.get(&key).is_some_and(|assigned| assigned == id)
                    })
                    .map(|(ix, path)| {
                        grouped_paths.insert(path.clone());
                        (ix, path.clone())
                    })
                    .collect();
                let id = id.to_owned();
                let toggle = id.clone();
                let pill = format!("group:{id}");
                let rename =
                    self.rename_row(&session_groups::RenameTarget::ProjectGroup(id.clone()), cx);
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
                    .relative()
                    .child(self.liquid.row(pill, false))
                    .hover(|s| s.text_color(content(1.)))
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
                    .child(rename.unwrap_or_else(|| row.into_any_element()));
                if collapsed {
                    hidden_projects.extend(members.into_iter().map(|(_, path)| path));
                } else {
                    section = section.child(
                        div().p_1().flex().flex_col().gap_px().children(
                            members
                                .into_iter()
                                .map(|(ix, path)| self.classic_project_row(ix, path.clone(), cx)),
                        ),
                    );
                }
                group_rows.push(section.into_any_element());
            }
        }

        // Keep only rendered project rows. Session activity remains authoritative
        // in project_sessions; revealing a row samples its current state anew.
        self.retain_project_marks(
            |path| {
                projects.iter().any(|project| project == path) && !hidden_projects.contains(path)
            },
            cx,
        );
        let liquid = self.liquid.clone();
        let bots = self.bots.clone().map(|bots| {
            bots.update(cx, |bots, cx| {
                div()
                    .child(bots.rail(None, &liquid, cx))
                    .child(bots.classic_channels_rail(&liquid, cx))
                    .into_any_element()
            })
        });
        let navigation = self.sidebar_navigation(cx);
        rail = rail
            .child(
                div()
                    .id("classic-project-rail-scroll")
                    .debug_selector(|| "classic-project-rail-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pb_2()
                    // Share the existing rail scroller so a short window can
                    // always reach navigation as well as the Working card/footer.
                    .child(navigation)
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
                            icon_button_hover("classic-open-project", "Open project", "plus", 0.)
                                .debug_selector(|| "classic-open-project".into())
                                .focusable()
                                .tab_stop(true)
                                .size(px(20.))
                                .relative()
                                .child(liquid.row("add:project", false))
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
                                .map(|(ix, path)| self.classic_project_row(ix, path.clone(), cx)),
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
        let _ = self.workspace.migrate_project_rail();
        let Some(group) = self
            .workspace
            .project_rail_state
            .as_ref()
            .and_then(|rail| rail.groups.iter().find(|g| g.id == id))
        else {
            return;
        };
        let collapsed = self
            .workspace
            .project_group_collapsed
            .get(id)
            .copied()
            .unwrap_or(group.collapsed);
        self.workspace
            .project_group_collapsed
            .insert(id.to_owned(), !collapsed);
        self.invalidate_chrome(cx);
        self.persist(cx);
        cx.notify();
    }

    fn classic_project_row(
        &mut self,
        ix: usize,
        path: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A project's terminal tabs belong to it, so they keep it selected.
        let selected = self.workspace.project == path
            && self.app_search.is_none()
            && self.active_app_view.is_none()
            && !self.bot_active(cx);
        let color = self.appearance(&path).color(&path);
        let menu = path.clone();
        let options = path.clone();
        let sessions = self.project_sessions(&path, cx);
        let working = sessions
            .iter()
            .filter(|(_, status)| *status == live_agents::Status::Working)
            .count();
        // The project's mark is the working grid in its colour, waving while
        // any of its sessions works.
        let mark = self
            .project_marks
            .entry(path.clone())
            .or_insert_with(|| cx.new(|_| ProjectMark::new(color)))
            .clone();
        mark.update(cx, |mark, cx| mark.set(color, working > 0, cx));
        let tree = self.project_tree(&sessions, cx);
        let row = div()
            .id(("classic-project", ix))
            .debug_selector(move || format!("classic-project-{ix}"))
            .role(Role::Button)
            .aria_label(format!(
                "Project {}{}",
                self.project_label(&path),
                if working > 0 { ", working" } else { "" }
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
            .child(self.liquid.row(format!("project:{path}"), selected))
            .child(
                div()
                    .size(px(16.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(mark),
            )
            .hover(|s| s.opacity(1.))
            .focus(|s| s.opacity(1.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .debug_selector(move || format!("classic-project-label-{ix}"))
                    .group_hover("classic-project-row", |s| s.pr(px(24.)))
                    .child(self.project_label(&path)),
            )
            // How many of its sessions are working.
            .when(working > 0, |d| {
                d.child(
                    div()
                        .debug_selector(move || format!("classic-project-working-{ix}"))
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .child(working.to_string()),
                )
            })
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
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_project(path.clone(), window, cx)
            }));
        div()
            .flex()
            .flex_col()
            .gap_px()
            .child(row)
            .children(tree)
            .into_any_element()
    }

    /// The project's running, approval-blocked and unseen finished sessions.
    fn project_sessions(&self, path: &str, cx: &App) -> Vec<(i64, live_agents::Status)> {
        let ids: HashSet<_> = self
            .sessions
            .keys()
            .chain(self.remote_marks.keys())
            .chain(self.unseen_finished.iter())
            .copied()
            .filter(|id| self.session_summaries.contains_key(id))
            .collect();
        let mut sessions: Vec<_> = ids
            .into_iter()
            .filter(|&id| self.session_metadata(id, cx).summary.cwd == path)
            .filter_map(|id| Some((id, self.sidebar_status(id, cx)?)))
            .collect();
        sessions.sort_by_key(|(id, _)| *id);
        sessions
    }

    /// A project's sessions hang under it like a Git tree.
    fn project_tree(
        &self,
        sessions: &[(i64, live_agents::Status)],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if !self.workspace.sidebar_activity_visible || !self.workspace.preferences.working_agents {
            return Vec::new();
        }
        let line = content(0.15);
        sessions
            .iter()
            .enumerate()
            .map(|(ix, &(id, status))| {
                let last = ix + 1 == sessions.len();
                let open = self.workspace.active == Some(id)
                    && self.active_app_view.is_none()
                    && self.app_search.is_none()
                    && !self.bot_active(cx);
                let ink = match status {
                    live_agents::Status::Approval => crate::theme::status(StatusColor::Warning, 1.),
                    live_agents::Status::Done => crate::theme::status(StatusColor::Success, 1.),
                    live_agents::Status::Working => content(0.5),
                };
                div()
                    .id(("project-session", id as usize))
                    .debug_selector(move || format!("project-session-{id}"))
                    .role(Role::Button)
                    .aria_label(self.session_metadata(id, cx).display_title())
                    .relative()
                    .h(px(28.))
                    .pl(px(30.))
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    // Indented past the tree lines, as a child of the project.
                    .child(
                        self.liquid
                            .row(format!("session:{id}"), false)
                            .left(px(24.)),
                    )
                    // The tree: a rail down from the project's mark, and a stub.
                    .child(
                        div()
                            .absolute()
                            .left(px(15.))
                            .top_0()
                            .w_px()
                            .map(|d| if last { d.h(px(14.)) } else { d.bottom_0() })
                            .bg(line),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(15.))
                            .top(px(14.))
                            .w(px(8.))
                            .h_px()
                            .bg(line),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(content(if open { 1. } else { 0.6 }))
                            .hover(|s| s.text_color(content(1.)))
                            .child(self.session_metadata(id, cx).display_title()),
                    )
                    .when(status != live_agents::Status::Working, |d| {
                        d.child(icon(status.glyph(), 12.).text_color(ink))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected_sessions.clear();
                        this.selection_anchor = Some(id);
                        this.open(id, window, cx);
                    }))
                    .into_any_element()
            })
            .collect()
    }

    fn classic_rail_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_shrink_0()
            // Settings' Back row keeps the same bottom padding.
            .px_2()
            .pb_2()
            .flex()
            .flex_col()
            // Sidebar options: a thin row above Settings; its menu opens upward.
            // It opens nothing in the pane, so it has a plain hover, not a pill.
            .child(
                div()
                    .id("classic-sidebar-options")
                    .debug_selector(|| "sidebar-options".into())
                    .role(Role::Button)
                    .aria_label("Sidebar options")
                    .focusable()
                    .tab_stop(true)
                    .relative()
                    .h(px(20.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(content(0.5))
                    .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                    .child(self.sidebar_options_anchor.probe())
                    .child(icon("more-horizontal", 14.))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_menu(
                            MenuTarget::SidebarOptions,
                            Anchor::Slot(this.sidebar_options_anchor.clone()),
                            window,
                            cx,
                        );
                    })),
            )
            .child(
                div()
                    .id("classic-rail-settings")
                    .debug_selector(|| "classic-rail-settings".into())
                    .role(Role::Button)
                    .aria_label("Settings")
                    .focusable()
                    .tab_stop(true)
                    .h(px(32.))
                    .px_2()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(14.))
                    .text_color(content(0.5))
                    .relative()
                    .child(self.liquid.row("settings-footer", false))
                    .hover(|s| s.text_color(content(1.)))
                    .child(icon("settings", 16.))
                    .child(div().flex_1().child("Settings"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings(crate::settings::Section::General, window, cx)
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
    fn hidden_project_rows_stop_ticks_and_resample_work_on_reveal(cx: &mut TestAppContext) {
        use std::time::Duration;
        crate::clock::use_test_clock(cx);
        let (window, _, session) = crate::shell::tests::window(cx);
        let path = "/work/vorkspace";
        window
            .update(cx, |shell, _, cx| {
                let rail = shell.workspace.project_rail_mut();
                rail.groups.push(crate::workspace::ProjectRailGroup {
                    id: "work".into(),
                    name: "Work".into(),
                    collapsed: false,
                });
                rail.assignments
                    .insert(vorkspace_core::files::path_key(path), "work".into());
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        for mode in ["compact", "hidden", "settings", "group"] {
            window
                .update(cx, |shell, _, cx| {
                    shell.sessions[&session].update(cx, |view, cx| {
                        view.busy = true;
                        cx.emit(SessionChanged);
                        cx.notify();
                    });
                })
                .unwrap();
            cx.run_until_parked();
            let mark = window
                .update(cx, |shell, _, cx| {
                    assert_eq!(
                        shell.sidebar_status(session, cx),
                        Some(live_agents::Status::Working)
                    );
                    shell.project_marks[path].clone()
                })
                .unwrap();
            cx.executor().advance_clock(Duration::from_millis(33));
            cx.run_until_parked();
            assert!(mark.read_with(cx, |mark, _| mark.animation_state().0) > 0);
            // Hide and reveal while still working, then hide again and finish.
            for finish_hidden in [false, true] {
                let mark = window
                    .update(cx, |shell, _, _| shell.project_marks[path].clone())
                    .unwrap();
                window
                    .update(cx, |shell, w, cx| {
                        match mode {
                            "compact" | "hidden" => {
                                shell.workspace.preferences.collapsed_project_rail = mode.into();
                                shell.workspace.project_rail_open = false;
                            }
                            "settings" => {
                                shell.open_settings(crate::settings::Section::General, w, cx)
                            }
                            "group" => {
                                shell
                                    .workspace
                                    .project_group_collapsed
                                    .insert("work".into(), true);
                            }
                            _ => unreachable!(),
                        }
                        shell.invalidate_chrome(cx);
                        cx.notify();
                    })
                    .unwrap();
                cx.run_until_parked();
                let stopped = mark.read_with(cx, |mark, _| mark.animation_state());
                assert!(!stopped.1, "{mode} must cancel the retained mark's task");
                window
                    .update(cx, |shell, _, cx| {
                        assert!(!shell.project_marks.contains_key(path), "{mode}");
                        assert_eq!(
                            shell.sidebar_status(session, cx),
                            Some(live_agents::Status::Working)
                        );
                        if finish_hidden {
                            shell.sessions[&session].update(cx, |view, cx| {
                                view.busy = false;
                                cx.emit(SessionChanged);
                                cx.notify();
                            });
                        }
                    })
                    .unwrap();
                cx.run_until_parked();
                cx.executor().advance_clock(Duration::from_millis(330));
                cx.run_until_parked();
                assert_eq!(
                    mark.read_with(cx, |mark, _| mark.animation_state()),
                    stopped,
                    "{mode}: hidden timer must never advance"
                );
                window
                    .update(cx, |shell, w, cx| {
                        if shell.settings.is_some() {
                            shell.close_settings(w, cx);
                        }
                        shell.workspace.project_rail_open = true;
                        shell
                            .workspace
                            .project_group_collapsed
                            .insert("work".into(), false);
                        shell.invalidate_chrome(cx);
                        cx.notify();
                    })
                    .unwrap();
                cx.run_until_parked();
                window
                    .update(cx, |shell, _, cx| {
                        assert_eq!(
                            shell.project_marks[path].read(cx).animation_state().1,
                            !finish_hidden,
                            "{mode}: reveal must use live work state"
                        );
                        assert_eq!(shell.sessions[&session].read(cx).busy, !finish_hidden);
                    })
                    .unwrap();
            }
        }
    }

    #[gpui::test]
    fn native_group_creation_rename_assignment_and_pin_render_and_persist(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            super::super::bind_keys(cx);
            crate::input::bind(cx);
        });
        let (window, _, _) = crate::shell::tests::window(cx);
        let path = "/work/another-project".to_string();
        window
            .update(cx, |shell, w, cx| {
                shell.run_menu(
                    MenuTarget::Project {
                        path: path.clone(),
                        rail: true,
                    },
                    "project-group:new",
                    None,
                    w,
                    cx,
                );
            })
            .unwrap();
        let group_id = window
            .update(cx, |shell, _, _| {
                shell.workspace.project_rail_state.as_ref().unwrap().groups[0]
                    .id
                    .clone()
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(1400.), px(1000.)));
        visual.run_until_parked();
        visual.simulate_input("Work");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        let selector: &'static str =
            Box::leak(format!("classic-project-group-{group_id}").into_boxed_str());
        assert!(visual.debug_bounds(selector).is_some());
        window
            .update(visual, |shell, w, cx| {
                assert_eq!(
                    shell.workspace.project_rail_state.as_ref().unwrap().groups[0].name,
                    "Work"
                );
                shell.run_menu(
                    MenuTarget::Project {
                        path: path.clone(),
                        rail: true,
                    },
                    "pin",
                    None,
                    w,
                    cx,
                );
                assert!(
                    shell
                        .workspace
                        .project_rail_state
                        .as_ref()
                        .unwrap()
                        .pinned
                        .contains(&vorkspace_core::files::path_key(&path))
                );
                shell.run_menu(
                    MenuTarget::Project {
                        path: path.clone(),
                        rail: true,
                    },
                    "pin",
                    None,
                    w,
                    cx,
                );
                shell.run_menu(
                    MenuTarget::Project {
                        path: path.clone(),
                        rail: true,
                    },
                    "project-group:set:",
                    None,
                    w,
                    cx,
                );
                shell.run_menu(
                    MenuTarget::Project {
                        path: path.clone(),
                        rail: true,
                    },
                    &format!("project-group:set:{group_id}"),
                    None,
                    w,
                    cx,
                );
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                let state = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
                let reopened = Workspace::restore(Some(&state), &[], &path);
                let rail = reopened.project_rail_state.unwrap();
                assert!(!rail.pinned.contains(&path));
                assert_eq!(
                    rail.assignments.get(&path).map(String::as_str),
                    Some(group_id.as_str())
                );
                assert_eq!(rail.groups[0].name, "Work");
            })
            .unwrap();
        window
            .update(visual, |shell, w, cx| {
                shell.run_menu(
                    MenuTarget::Project {
                        path: path.clone(),
                        rail: true,
                    },
                    &format!("group-remove:{group_id}"),
                    None,
                    w,
                    cx,
                );
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
                let restored = Workspace::restore(Some(&saved), &[], &path);
                let rail = restored.project_rail_state.unwrap();
                assert!(rail.groups.is_empty());
                assert!(rail.assignments.is_empty());
            })
            .unwrap();
    }

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
                shell.workspace.project_rail_state = None;
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
