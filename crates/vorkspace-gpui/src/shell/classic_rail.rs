//! The original project rail is independent of the project's session/file pane.
use super::*;
use vorkspace_ui::radius;

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
            self.workspace
                .project_rail_width
                .max(self.open_rail_floor())
        } else if self.workspace.preferences.collapsed_project_rail == "hidden" {
            0.
        } else {
            COMPACT_WIDTH
        }
    }

    /// The open rail holds all of navigation. At low zoom the native traffic
    /// lights take more logical width than the saved rail, and an overhang
    /// would push the page below a second title row.
    pub(super) fn open_rail_floor(&self) -> f32 {
        let lights = traffic_light_inset(
            self.workspace.preferences.interface_scale(),
            self.fullscreen,
        );
        // Each button rounds to whole device pixels: at most half a pixel more
        // at 1x (`navigation_button_size`).
        (lights + 2. * 24.5 + 6.).ceil()
    }

    /// Navigation always overhangs the icon rail, under a rule across the pane
    /// or a left sidebar. The divider turns into that rule rather than
    /// meeting it at a right angle.
    pub(super) fn rail_meets_title_rule(&self) -> bool {
        self.workspace.classic_layout
            && !self.workspace.project_rail_open
            && self.settings.is_none()
            && self.classic_rail_width() > 0.
    }

    pub(super) fn toggle_project_rail(&mut self, cx: &mut Context<Self>) {
        self.workspace.project_rail_open = !self.workspace.project_rail_open;
        self.project_rail_drag = None;
        self.close_menu(cx);
        self.invalidate_chrome(cx);
        self.persist(cx);
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
            .child({
                // Around its neighbour's rounded corners (the pane, or a left
                // sidebar). Beside the icon rail, navigation overhangs the
                // pane, so it turns into the title bar's rule instead.
                let cap_top = if self.workspace.project_rail_open || self.settings.is_some() {
                    Some(0.)
                } else {
                    self.rail_meets_title_rule().then_some(TITLE_BAR_HEIGHT)
                };
                crate::panel_kit::rail_divider(cap_top)
            })
            .child(div().h(px(TITLE_BAR_HEIGHT)).flex_shrink_0())
            .child(crate::avatar::sidebar_cursor_tracker());

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
                    &settings.read(cx).visible_sections(),
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
        self.reconcile_vorkspace_bots(cx);

        if !self.workspace.project_rail_open {
            return self.compact_project_rail(rail, cx);
        }
        // Expanded, sessions hang under their projects instead of the Working card.
        self.live_agents.update(cx, |activity, _| activity.hide());

        let _ = self.workspace.migrate_project_rail();
        // The rail lists the vorkspace's sessions rather than its projects;
        // projects are picked from the sidebar header within the vorkspace.
        self.retain_project_marks(|_| false, cx);
        let bots = self.bots.clone().map(|bots| {
            bots.update(cx, |bots, cx| {
                div().child(bots.rail(None, cx)).into_any_element()
            })
        });
        let navigation = self.sidebar_navigation(cx);
        let switcher = self.vorkspace_switcher(cx);
        let sessions = self.rail_sessions_section(cx);
        rail = rail
            .child(switcher)
            .child(
                // Navigation and Bots hold their place; the session list
                // takes the remaining height and scrolls on its own.
                div()
                    .id("classic-project-rail-scroll")
                    .debug_selector(|| "classic-project-rail-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .pb_2()
                    .child(div().flex_shrink_0().child(navigation))
                    .children(bots.map(|b| div().flex_shrink_0().child(b)))
                    .child(sessions),
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
                                Some((event.position.x, this.classic_rail_width()));
                            cx.stop_propagation();
                        }),
                    ),
            );
        rail.into_any_element()
    }

    fn classic_rail_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_shrink_0()
            // Settings' Back row keeps the same bottom padding.
            .px_2()
            .pb_2()
            .flex()
            .flex_col()
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
                    .rounded(px(radius::CONTROL))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(14.))
                    .text_color(content(0.5))
                    .relative()
                    .hover(|s| {
                        s.bg(vorkspace_ui::fluid::hover(0.08))
                            .text_color(content(1.))
                    })
                    .focus(|s| s.bg(content(0.1)).text_color(content(1.)))
                    .child(icon("settings", 16.))
                    .child(div().flex_1().child("Settings"))
                    // Sidebar options ride at the row's end rather than on a
                    // row of their own; the menu opens upward from here.
                    .child(
                        div()
                            .id("classic-sidebar-options")
                            .debug_selector(|| "sidebar-options".into())
                            .role(Role::Button)
                            .aria_label("Sidebar options")
                            .focusable()
                            .tab_stop(true)
                            .relative()
                            .size(px(24.))
                            .flex_shrink_0()
                            .rounded(px(radius::CONTROL))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(content(0.45))
                            .hover(|s| {
                                s.bg(vorkspace_ui::fluid::hover(0.08))
                                    .text_color(content(1.))
                            })
                            .child(self.sidebar_options_anchor.probe())
                            .child(icon("more-horizontal", 14.))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_menu(
                                    MenuTarget::SidebarOptions,
                                    Anchor::Slot(this.sidebar_options_anchor.clone()),
                                    window,
                                    cx,
                                );
                            })),
                    )
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
        let hidden_bots = self.workspace.hidden_bots();
        let (_, vorkspace_name) = self
            .workspace
            .active_vorkspace()
            .map(|v| (v.id.clone(), v.name.clone()))
            .unwrap_or_default();
        // Every Bot in the vorkspace, as the expanded rail lists them.
        let pinned_bots: Vec<_> = {
            self.bots
                .as_ref()
                .filter(|_| self.workspace.preferences.bots)
                .map(|manager| {
                    let manager = manager.read(cx);
                    manager
                        .roster
                        .bots
                        .iter()
                        .filter(|bot| !bot.is_minion() && !hidden_bots.contains(&bot.id))
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
                                .rounded(px(radius::CONTROL))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(selected, |d| d.bg(content(0.1)))
                                .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
                                .child(crate::avatar::bot_avatar(bot, "idle", 18., true))
                                .on_click(move |_, window, cx| {
                                    manager.update(cx, |manager, cx| manager.open(&id, window, cx));
                                })
                                .into_any_element()
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        // The Sessions button marks finished replies or work in progress
        // anywhere in the vorkspace.
        let listed = self.sidebar_session_ids(cx);
        let unread = listed.iter().any(|id| self.unseen_finished.contains(id));
        let working = listed.iter().any(|id| {
            let meta = self.session_metadata(*id, cx);
            meta.responding() || meta.needs_approval()
        });
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
                // The vorkspace leads the icon rail as it does the full one.
                .child(
                    div()
                        .id("classic-compact-vorkspace")
                        .debug_selector(|| "classic-compact-vorkspace".into())
                        .role(Role::Button)
                        .aria_label(format!("Vorkspace {vorkspace_name}. Switch vorkspace"))
                        .map(vorkspace_ui::theme::tab_reachable)
                        .size(px(32.))
                        .rounded(px(radius::CONTROL))
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
                        .child(super::vorkspaces::vorkspace_mark(
                            &self
                                .workspace
                                .active_vorkspace()
                                .cloned()
                                .unwrap_or_default(),
                            20.,
                        ))
                        .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                            this.open_menu(
                                MenuTarget::Vorkspace,
                                Anchor::Point(event.position()),
                                window,
                                cx,
                            );
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
                                    "Codebase" => this.open_codebase(window, cx),
                                    "Automations" => this.open_automations(window, cx),
                                    "To-dos" => this.open_todos(window, cx),
                                    _ => (),
                                },
                            ))
                        }),
                )
                // Then the vorkspace's Bots and its Sessions, in the expanded
                // rail's order. Sessions opens the rail itself.
                .when(!pinned_bots.is_empty(), |d| {
                    d.child(div().h_px().w(px(24.)).bg(content(0.08)))
                        .child(div().flex().flex_col().gap_1p5().children(pinned_bots))
                })
                .child(div().h_px().w(px(24.)).bg(content(0.08)))
                .child(
                    icon_button("classic-compact-sessions", "Sessions", "message-multiple")
                        .debug_selector(|| "classic-compact-sessions".into())
                        .aria_label(if unread {
                            "Sessions, new replies"
                        } else if working {
                            "Sessions, working"
                        } else {
                            "Sessions"
                        })
                        .focusable()
                        .tab_stop(true)
                        .size(px(32.))
                        .relative()
                        .when(unread || working, |d| {
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
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.workspace.project_rail_open {
                                this.toggle_project_rail(cx);
                            }
                        })),
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
