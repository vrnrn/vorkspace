//! Filtered, virtualized session cards and their rail interactions.
use super::*;
use vorkspace_ui::radius;

/// Collapsed, the rail shows this many sessions: working and
/// approval-blocked ones first, then the latest.
const RAIL_SESSIONS: usize = 5;
const RAIL_CARD_HEIGHT: f32 = 46.;
const RAIL_ROW_HEIGHT: f32 = RAIL_CARD_HEIGHT + 1.;

struct RailSession {
    title: String,
    provider: String,
    project: String,
    busy: bool,
    needs_input: bool,
    pinned: bool,
    archived: bool,
    updated_at: u64,
}

impl Shell {
    fn rail_sessions(
        &self,
        launches: &HashMap<i64, &vorkspace_core::bot::Bot>,
        cx: &App,
    ) -> (Vec<i64>, usize) {
        let mut sessions = self.filtered_rail_sessions(launches, cx);
        let total = sessions.len();
        let narrowed = !self.sidebar.query.is_empty() || self.session_filters_active(cx);
        // Expanded, searching or filtering, every match is listed.
        if !(self.rail.sessions_expanded || narrowed) && sessions.len() > RAIL_SESSIONS {
            let renaming = self
                .sidebar
                .renaming
                .as_ref()
                .map(|edit| edit.target.anchor());
            let extra = renaming.and_then(|id| {
                sessions[RAIL_SESSIONS..]
                    .iter()
                    .position(|session| *session == id)
                    .map(|ix| sessions.remove(RAIL_SESSIONS + ix))
            });
            sessions.truncate(RAIL_SESSIONS);
            if let Some(session) = extra {
                sessions.push(session);
            }
        }
        (sessions, total)
    }

    pub(super) fn close_rail_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rail.search_open = false;
        self.sidebar.query.clear();
        self.sidebar_search
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.focus_active(window, cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }

    /// The sessions the rail shows, in its order, for stepping and ranges.
    pub(super) fn rail_session_ids(&self, cx: &App) -> Vec<i64> {
        self.rail_sessions(&self.session_launchers(cx), cx).0
    }

    pub(super) fn rail_sessions_min_height(&self) -> f32 {
        if self.rail.search_open || !self.sidebar.query.is_empty() {
            126.
        } else {
            94.
        }
    }

    /// Sessions across the vorkspace's projects: running first, then the latest.
    pub(super) fn rail_sessions_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let launches = self.session_launchers(cx);
        let (sessions, total) = self.rail_sessions(&launches, cx);
        let launcher_ids = launches
            .into_iter()
            .map(|(id, bot)| (id, bot.id.clone()))
            .collect::<HashMap<_, _>>();
        let hovered = self.preview.session_hover;
        self.sidebar
            .session_row_bounds
            .retain(|id, _| Some(*id) == hovered);
        if self.rail.list_ids.as_slice() != sessions.as_slice() {
            let mut offset = self.rail.list.logical_scroll_top();
            let anchor = self.rail.list_ids.get(offset.item_ix);
            self.rail
                .list
                .reset_with_uniform_height(sessions.len(), px(RAIL_ROW_HEIGHT));
            if (offset.item_ix != 0 || offset.offset_in_item > px(0.))
                && let Some(ix) = anchor.and_then(|id| sessions.iter().position(|row| row == id))
            {
                offset.item_ix = ix;
                self.rail.list.scroll_to(offset);
            }
            self.rail.list_ids = std::rc::Rc::new(sessions);
        }
        let sessions = self.rail.list_ids.clone();
        let owner = cx.weak_entity();
        self.rail.list.set_scroll_handler(move |_, _, cx| {
            let _ = owner.update(cx, |this, cx| {
                this.preview.last_scroll = Some(vorkspace_ui::clock::Instant::now());
                this.close_preview(cx);
            });
        });
        let expanded = self.rail.sessions_expanded;
        let more = total > sessions.len() || expanded;
        let filtering = self.session_filters_active(cx);
        let searching = self.rail.search_open || !self.sidebar.query.is_empty();
        let search = searching.then(|| {
            div()
                .mx_2()
                .mb_1()
                .h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded(px(radius::CONTROL))
                .bg(content(0.05))
                .child(icon("search", 12.).text_color(content(0.5)))
                .child(
                    div()
                        .debug_selector(|| "rail-sessions-search-field".into())
                        .flex_1()
                        .min_w_0()
                        .child(self.sidebar_search.clone()),
                )
                .child(
                    icon_button("rail-sessions-search-close", "Close search", "close")
                        .size(px(18.))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_rail_search(window, cx)),
                        ),
                )
        });
        let heading = section_label("Sessions").mx_2().child(
            div()
                .flex()
                .items_center()
                .gap_0p5()
                .child(
                    icon_button("rail-sessions-search", "Search sessions", "search")
                        .debug_selector(|| "rail-sessions-search".into())
                        .size(px(20.))
                        .when(searching, |d| d.bg(content(0.1)))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.rail.search_open {
                                this.close_rail_search(window, cx);
                            } else {
                                this.rail.search_open = true;
                                window.focus(&this.sidebar_search.focus_handle(cx), cx);
                                this.invalidate_chrome(cx);
                                cx.notify();
                            }
                        })),
                )
                .child(
                    icon_button("rail-sessions-filter", "Filter sessions", "filter")
                        .debug_selector(|| "rail-sessions-filter".into())
                        .size(px(20.))
                        .relative()
                        .when(
                            filtering || self.menu_open(&MenuTarget::SessionFilter),
                            |d| d.bg(content(0.1)),
                        )
                        .child(self.sidebar.filter_anchor.probe())
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.menu_open(&MenuTarget::SessionFilter) {
                                this.close_menu(cx);
                            } else {
                                this.open_menu(
                                    MenuTarget::SessionFilter,
                                    Anchor::Slot(this.sidebar.filter_anchor.clone()),
                                    window,
                                    cx,
                                );
                            }
                        })),
                )
                .child(
                    if self.workspace.project.is_empty() {
                        icon_button(
                            "rail-sessions-new",
                            "Open project to start a session",
                            "folder-open",
                        )
                    } else {
                        icon_button("rail-sessions-new", "New session", "plus")
                    }
                    .debug_selector(|| "rail-sessions-new".into())
                    .size(px(20.))
                    .on_click(cx.listener(|this, _, _, cx| this.create(this.default_kind(), cx))),
                ),
        );
        // Below the list, the disclosure reaches further back.
        // A full-width disclosure under the list: all sessions, or back to five.
        let disclosure = (more && !filtering && self.sidebar.query.is_empty()).then(|| {
            div().flex_shrink_0().px_2().pt_1().child(
                div()
                    .id("rail-sessions-more")
                    .debug_selector(|| "rail-sessions-more".into())
                    .role(Role::Button)
                    .aria_label(if expanded {
                        "Show fewer sessions"
                    } else {
                        "Show all sessions"
                    })
                    .aria_expanded(expanded)
                    .map(vorkspace_ui::theme::tab_reachable)
                    .w_full()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::CONTROL))
                    .cursor_pointer()
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.06)))
                    .child(
                        icon(
                            if expanded {
                                "chevron-up"
                            } else {
                                "chevron-down"
                            },
                            14.,
                        )
                        .text_color(content(0.5)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.rail.sessions_expanded = !this.rail.sessions_expanded;
                        this.invalidate_chrome(cx);
                        cx.notify();
                    })),
            )
        });
        let now = vorkspace_core::time::now_ms();
        div()
            .id("rail-sessions")
            .debug_selector(|| "rail-sessions".into())
            // The session list owns selection keys: F2 renames, Delete removes.
            .key_context("SessionSidebar")
            .track_focus(&self.sidebar_focus)
            .on_action(cx.listener(
                |this, _: &super::session_groups::RenameSidebarItem, window, cx| {
                    this.rename_sidebar_item(window, cx)
                },
            ))
            .on_action(cx.listener(
                |this, _: &super::session_groups::DeleteSidebarSelection, window, cx| {
                    this.delete_sidebar_selection(window, cx)
                },
            ))
            .flex_1()
            .min_h(px(self.rail_sessions_min_height()))
            .flex()
            .flex_col()
            .child(heading)
            .children(search)
            .child(if sessions.is_empty() {
                div()
                    .mx_2()
                    .px_2()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child(if self.workspace.project.is_empty() {
                        "Open a project to start a session"
                    } else if filtering || !self.sidebar.query.is_empty() {
                        "No sessions match"
                    } else {
                        "No sessions yet · ⌘N starts one"
                    })
                    .into_any_element()
            } else {
                let count = sessions.len();
                let owner = cx.weak_entity();
                div()
                    .id("rail-sessions-list")
                    .debug_selector(|| "rail-sessions-list".into())
                    .h(px(count as f32 * RAIL_ROW_HEIGHT))
                    .flex_shrink_1()
                    .min_h(px(RAIL_CARD_HEIGHT))
                    .px_2()
                    .child(
                        gpui::list(self.rail.list.clone(), move |ix, _, cx| {
                            let id = sessions[ix];
                            owner
                                .update(cx, |this, cx| {
                                    div()
                                        .pb(px(1.))
                                        .child(this.rail_session_card(
                                            id,
                                            launcher_ids.get(&id),
                                            now,
                                            cx,
                                        ))
                                        .into_any_element()
                                })
                                .unwrap_or_else(|_| div().into_any_element())
                        })
                        .size_full(),
                    )
                    .into_any_element()
            })
            .children(disclosure)
            .into_any_element()
    }

    fn rail_session_card(
        &mut self,
        id: i64,
        launcher: Option<&String>,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let meta = self.session_metadata(id, cx);
        let folder = self
            .sidebar
            .groups
            .get(&meta.summary.cwd)
            .and_then(|folders| folders.folder_of(id));
        let s = RailSession {
            title: meta.display_title().to_string(),
            provider: meta.summary.provider.clone(),
            project: self.project_label(&meta.summary.cwd),
            busy: meta.responding(),
            needs_input: meta.needs_approval(),
            pinned: meta.summary.pinned,
            archived: meta.summary.archived,
            updated_at: meta.summary.updated_at,
        };
        if let Some(row) = self.rename_row(id, cx) {
            return row;
        }
        let detail = match folder {
            Some(folder) => format!("{} · {}", folder.name, s.project),
            None => s.project.clone(),
        };
        let scope = folder.map_or_else(
            || s.project.clone(),
            |folder| format!("{} · Folder {}", s.project, folder.name),
        );
        let filed = folder.is_some();
        let active = self.sidebar.selected_sessions.contains(&id)
            || (self.workspace.active == Some(id) && !self.bot_active(cx) && !self.app_view_open());
        let anchor = self
            .sidebar
            .session_row_bounds
            .entry(id)
            .or_default()
            .clone();
        let menu_anchor = anchor.clone();
        // Status only: card actions live in the right-click menu,
        // so hovering never changes a card's size.
        let status = if s.needs_input {
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_color(vorkspace_ui::theme::warning())
                .child(icon("circle-alert", 12.).text_color(vorkspace_ui::theme::warning()))
                .child("Approval")
                .into_any_element()
        } else if s.busy {
            div()
                .text_color(accent_color())
                .child("Working…")
                .into_any_element()
        } else {
            div()
                .text_color(content(0.45))
                .when(s.archived, |d| d.child("Archived · "))
                .child(vorkspace_core::time::relative(s.updated_at, now))
                .into_any_element()
        };
        let bot = launcher
            .and_then(|id| self.bots.as_ref()?.read(cx).roster.get(id))
            .map(|bot| {
                (
                    crate::avatar::bot_avatar(bot, "idle", 12., true),
                    bot.label(),
                )
            });
        let state = if s.needs_input {
            "Needs approval"
        } else if s.busy {
            "Working"
        } else if s.archived {
            "Archived"
        } else {
            "Idle"
        };
        div()
            .id(("rail-session", id as usize))
            .debug_selector(move || format!("rail-session-{id}"))
            .role(Role::Button)
            .aria_label(s.title.clone())
            .aria_description(format!(
                "{} · {} · {state}",
                scope,
                Kind::from_id(&s.provider).label()
            ))
            .aria_keyshortcuts("F2 Shift+F10")
            .aria_selected(active)
            .h(px(RAIL_CARD_HEIGHT))
            .map(vorkspace_ui::theme::tab_reachable)
            .px_2()
            .py(px(6.))
            .rounded(px(radius::CONTROL))
            .cursor_pointer()
            .when(active, |d| d.bg(content(0.1)))
            .when(!active, |d| {
                d.hover(|s| s.bg(vorkspace_ui::fluid::hover(0.06)))
            })
            .when(s.needs_input, |d| {
                d.border_1()
                    .border_color(vorkspace_ui::theme::warning().opacity(0.4))
            })
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .text_color(content(if active { 1. } else { 0.85 }))
                            .child(s.title),
                    )
                    .when(s.pinned, |d| {
                        d.child(icon("pin", 11.).flex_shrink_0().text_color(content(0.45)))
                    })
                    .children(self.work_item_badge(id, cx))
                    .child(div().flex_shrink_0().text_size(px(11.)).child(status)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(provider_icon(&s.provider, 12.))
                    .when(filed, |d| d.child(icon("folder", 11.).flex_shrink_0()))
                    .child(div().flex_1().min_w_0().truncate().child(detail))
                    .when_some(bot, |d, (avatar, label)| {
                        d.child(
                            div()
                                .debug_selector(move || format!("rail-session-bot-{id}"))
                                .flex_shrink_0()
                                .max_w(px(96.))
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(avatar)
                                .child(div().min_w_0().truncate().child(label)),
                        )
                    }),
            )
            .when(self.deleting.contains(&id), |d| d.opacity(0.5))
            .relative()
            .child(anchor.probe())
            .on_hover(cx.listener(move |this, hovered, window, cx| {
                this.hover_session(id, anchor.clone(), *hovered, window, cx);
                this.invalidate_chrome(cx);
            }))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if std::mem::take(&mut this.sidebar.skip_session_click) {
                        return;
                    }
                    this.select_sidebar_session(id, event.modifiers(), window, cx)
                }),
            )
            // A card dragged onto a pane edge opens there.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    this.sidebar.skip_session_click = false;
                    this.tabs.drag = None;
                    this.panes.drag = Some(PaneDrag {
                        source: DragSource::Session(id),
                        start: event.position,
                        position: event.position,
                        active: false,
                        over: None,
                        insert: None,
                    });
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_session_actions(id, Anchor::Point(event.position), window, cx);
                }),
            )
            .on_action(cx.listener(
                move |this, _: &super::session_groups::RenameSidebarItem, window, cx| {
                    this.start_rename(super::session_groups::RenameTarget::Session(id), window, cx);
                    cx.stop_propagation();
                },
            ))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    let key = &event.keystroke;
                    if key.key == "f10" && key.modifiers.shift {
                        this.open_session_actions(
                            id,
                            Anchor::Slot(menu_anchor.clone()),
                            window,
                            cx,
                        );
                    }
                }),
            )
            .into_any_element()
    }

    fn open_session_actions(
        &mut self,
        id: i64,
        anchor: Anchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.context_selection = !self.sidebar.selected_sessions.contains(&id);
        if self.sidebar.context_selection {
            self.sidebar.selected_sessions.clear();
            self.sidebar.selected_sessions.insert(id);
        }
        self.open_menu(MenuTarget::Session(id), anchor, window, cx);
        window.prevent_default();
        cx.stop_propagation();
    }
}
