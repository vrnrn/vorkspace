//! Vorkspaces in the window: the switcher atop the rail, the dialog that
//! edits a vorkspace's name, folders and Bots, and the rail's Sessions list.
//!
//! The model lives in `workspace::vorkspaces`; this file only presents it.
//! Switching never closes anything: it remembers the project the vorkspace
//! showed and lands on the target's last project, as a project click does.
use super::*;
use crate::popover::{MenuAction, MenuItem};
use gpui::Div;
use vorkspace_core::files::path_key;
use vorkspace_ui::radius;

pub(crate) struct VorkspaceDialog {
    pub(crate) editing: String,
    /// A project's own tab (Codebase, Notifications); none shows the
    /// vorkspace itself.
    pub(crate) project: Option<String>,
    pub(super) name: Entity<input::Input>,
    focus: FocusHandle,
    pub(super) confirm_delete: bool,
    _name: Subscription,
}

/// Collapsed, the rail shows this many sessions: working and
/// approval-blocked ones first, then the latest.
const RAIL_SESSIONS: usize = 5;

/// A stable tint for a vorkspace's initial, from its id.
fn vorkspace_hue(id: &str) -> f32 {
    let hash = id
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    (hash % 360) as f32 / 360.
}

pub(crate) fn vorkspace_mark(def: &crate::workspace::VorkspaceDef, side: f32) -> Div {
    let (id, name) = (def.id.as_str(), def.name.as_str());
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "V".into());
    div()
        .size(px(side))
        .flex_shrink_0()
        .rounded(px(vorkspace_ui::radius::CONTROL))
        .bg(match def.color {
            Some(color) => gpui::rgb(color).into(),
            None => gpui::hsla(vorkspace_hue(id), 0.55, 0.5, 1.),
        })
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(side * 0.55))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::white())
        .child(initial)
}

struct RailSession {
    id: i64,
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
    pub(super) fn switch_vorkspace(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if id == self.workspace.active_vorkspace
            || !self.workspace.vorkspaces.iter().any(|v| v.id == id)
        {
            return;
        }
        let from = self.workspace.active_vorkspace.clone();
        if !self.workspace.project.is_empty() {
            self.workspace
                .vorkspace_project
                .insert(from, self.workspace.project.clone());
        }
        self.workspace.active_vorkspace = id.to_owned();
        self.rail_sessions_expanded = false;
        self.reconcile_vorkspace_bots(cx);
        let landing = self.workspace.vorkspace_landing(id);
        self.open_project_from_user(landing, window, cx);
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn vorkspace_menu_items(&self) -> (SharedString, Vec<MenuItem>, f32, MenuStyle) {
        let mut items: Vec<MenuItem> = self
            .workspace
            .vorkspaces
            .iter()
            .map(|v| {
                MenuAction::new(format!("switch:{}", v.id), v.name.clone())
                    .checked(v.id == self.workspace.active_vorkspace)
                    .into()
            })
            .collect();
        items.extend([
            MenuItem::Separator,
            // No icon, so it lines up with the vorkspace names above.
            MenuAction::new("manage", "Manage vorkspaces").into(),
        ]);
        ("Vorkspaces".into(), items, 240., MenuStyle::Group)
    }

    pub(super) fn run_vorkspace_menu(
        &mut self,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = action.strip_prefix("switch:") {
            let id = id.to_owned();
            self.switch_vorkspace(&id, window, cx);
        } else if action == "manage" {
            self.open_vorkspace_dialog(None, window, cx);
        }
    }

    pub(super) fn open_vorkspace_dialog(
        &mut self,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = id.unwrap_or_else(|| self.workspace.active_vorkspace.clone());
        let name = self
            .workspace
            .vorkspaces
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.name.clone())
            .unwrap_or_default();
        let field = cx.new(|cx| {
            let mut field =
                input::Input::field("Vorkspace name", "Vorkspace name", content(0.35), cx);
            field.set_text(name, cx);
            field
        });
        let editing = id.clone();
        let sub = cx.subscribe(&field, move |this, field, _: &input::Changed, cx| {
            let name = field.read(cx).content.trim().to_owned();
            if name.is_empty() {
                return;
            }
            if let Some(v) = this.workspace.vorkspace_mut(&editing) {
                v.name = name;
                this.persist(cx);
                cx.notify();
            }
        });
        let focus = cx.focus_handle();
        window.focus(&field.focus_handle(cx), cx);
        self.vorkspace_dialog = Some(VorkspaceDialog {
            editing: id,
            project: None,
            name: field,
            focus,
            confirm_delete: false,
            _name: sub,
        });
        cx.notify();
    }

    pub(super) fn close_vorkspace_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.vorkspace_dialog.take().is_some() {
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    fn vorkspace_bots(&self, cx: &App) -> Vec<vorkspace_core::bot::Bot> {
        self.bots
            .as_ref()
            .map(|m| {
                m.read(cx)
                    .roster
                    .bots
                    .iter()
                    .filter(|b| !b.is_minion())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// New Bots join the active vorkspace; deleted ones leave every list.
    pub(super) fn reconcile_vorkspace_bots(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = self.bots.clone() else {
            return;
        };
        if !manager.read(cx).roster_loaded {
            return;
        }
        let ids: Vec<String> = self.vorkspace_bots(cx).into_iter().map(|b| b.id).collect();
        if self.workspace.assign_unowned_bots(&ids) {
            // Called while the rail renders: saving touches the chrome being
            // drawn, so it waits until the frame is done.
            let this = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = this.update(cx, |this, cx| this.persist(cx));
            });
        }
        let hidden = self.workspace.hidden_bots();
        manager.update(cx, |m, _| m.vorkspace_hidden = hidden);
    }

    /// The vorkspace switcher at the top of the rail.
    pub(super) fn vorkspace_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let (_, name) = self
            .workspace
            .active_vorkspace()
            .map(|v| (v.id.clone(), v.name.clone()))
            .unwrap_or_default();
        div()
            .px_2()
            .pb_1()
            .child(
                div()
                    .id("vorkspace-switcher")
                    .debug_selector(|| "vorkspace-switcher".into())
                    .role(Role::Button)
                    .aria_label(format!("Vorkspace {name}. Switch vorkspace"))
                    .map(vorkspace_ui::theme::tab_reachable)
                    .relative()
                    .h(px(36.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(radius::CONTROL))
                    .cursor_pointer()
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
                    .child(vorkspace_mark(
                        &self
                            .workspace
                            .active_vorkspace()
                            .cloned()
                            .unwrap_or_default(),
                        22.,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .child(icon("chevron-down", 12.).text_color(content(0.45)))
                    .child(self.vorkspace_anchor.probe())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_menu(
                            MenuTarget::Vorkspace,
                            Anchor::Slot(this.vorkspace_anchor.clone()),
                            window,
                            cx,
                        );
                    })),
            )
            .into_any_element()
    }

    fn rail_sessions(&self, cx: &App) -> (Vec<RailSession>, usize) {
        let mut sessions: Vec<RailSession> = self
            .sidebar_session_ids(cx)
            .into_iter()
            .map(|id| {
                let meta = self.session_metadata(id, cx);
                RailSession {
                    id,
                    title: meta.display_title().to_string(),
                    provider: meta.summary.provider.clone(),
                    project: self.project_label(&meta.summary.cwd),
                    busy: meta.responding(),
                    needs_input: meta.needs_approval(),
                    pinned: meta.summary.pinned,
                    archived: meta.summary.archived,
                    updated_at: meta.summary.updated_at,
                }
            })
            .collect();
        sessions
            .sort_by_key(|s| std::cmp::Reverse((s.busy || s.needs_input, s.pinned, s.updated_at)));
        let total = sessions.len();
        let narrowed = !self.query.is_empty() || self.session_filters_active(cx);
        // Expanded, searching or filtering, every match is listed.
        if !(self.rail_sessions_expanded || narrowed) && sessions.len() > RAIL_SESSIONS {
            let renaming = self.renaming.as_ref().map(
                |super::session_groups::Renaming {
                     target: super::session_groups::RenameTarget::Session(id),
                     ..
                 }| *id,
            );
            let extra = renaming.and_then(|id| {
                sessions[RAIL_SESSIONS..]
                    .iter()
                    .position(|s| s.id == id)
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
        self.rail_search_open = false;
        self.query.clear();
        self.sidebar_search
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.focus_active(window, cx);
        self.invalidate_chrome(cx);
        cx.notify();
    }

    /// The sessions the rail shows, in its order, for stepping and ranges.
    pub(super) fn rail_session_ids(&self, cx: &App) -> Vec<i64> {
        self.rail_sessions(cx).0.into_iter().map(|s| s.id).collect()
    }

    /// Sessions across the vorkspace's projects, borrowed from the Bot island:
    /// what is running first, else the latest few.
    pub(super) fn rail_sessions_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (sessions, total) = self.rail_sessions(cx);
        // Each card anchors its hover preview, as the sidebar rows did.
        let listed: HashSet<i64> = sessions.iter().map(|s| s.id).collect();
        self.session_row_bounds.retain(|id, _| listed.contains(id));
        for id in &listed {
            self.session_row_bounds.entry(*id).or_default();
        }
        let launcher: HashMap<i64, vorkspace_core::bot::Bot> = self
            .bots
            .as_ref()
            .map(|m| {
                m.read(cx)
                    .roster
                    .bots
                    .iter()
                    .flat_map(|bot| {
                        bot.launches
                            .iter()
                            .map(move |l| (l.session_id, bot.clone()))
                    })
                    .filter(|(id, _)| sessions.iter().any(|s| s.id == *id))
                    .collect()
            })
            .unwrap_or_default();
        let expanded = self.rail_sessions_expanded;
        let more = total > sessions.len() || expanded;
        let filtering = self.session_filters_active(cx);
        let searching = self.rail_search_open || !self.query.is_empty();
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
                            if this.rail_search_open {
                                this.close_rail_search(window, cx);
                            } else {
                                this.rail_search_open = true;
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
                        })),
                )
                .child(
                    icon_button("rail-sessions-new", "New session", "plus")
                        .debug_selector(|| "rail-sessions-new".into())
                        .size(px(20.))
                        .on_click(
                            cx.listener(|this, _, _, cx| this.create(this.default_kind(), cx)),
                        ),
                ),
        );
        // Below the list, the disclosure reaches further back.
        // A full-width disclosure under the list: all sessions, or back to five.
        let disclosure = (more && !filtering && self.query.is_empty()).then(|| {
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
                        this.rail_sessions_expanded = !this.rail_sessions_expanded;
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
            .min_h_0()
            .flex()
            .flex_col()
            .child(heading)
            .children(search)
            .child(
                div()
                    .id("rail-sessions-list")
                    .debug_selector(|| "rail-sessions-list".into())
                    // Sized by its cards, shrinking to scroll once the rail
                    // runs out of room, so the chevron stays under the last.
                    .flex_shrink(1.)
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .flex()
                    .flex_col()
                    .gap_px()
                    .when(sessions.is_empty(), |d| {
                        d.child(
                            div()
                                .px_2()
                                .py_1()
                                .text_size(px(12.))
                                .text_color(content(0.45))
                                .child(if filtering || !self.query.is_empty() {
                                    "No sessions match"
                                } else {
                                    "No sessions yet · ⌘N starts one"
                                }),
                        )
                    })
                    .children(sessions.into_iter().map(|s| {
                        let id = s.id;
                        if let Some(row) =
                            self.rename_row(&super::session_groups::RenameTarget::Session(id), cx)
                        {
                            return row;
                        }
                        let active = self.selected_sessions.contains(&id)
                            || (self.workspace.active == Some(id)
                                && !self.bot_active(cx)
                                && !self.app_view_open());
                        let anchor = self
                            .session_row_bounds
                            .get(&id)
                            .cloned()
                            .unwrap_or_default();
                        // Status only: card actions live in the right-click menu,
                        // so hovering never changes a card's size.
                        let status = if s.needs_input {
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_color(vorkspace_ui::theme::warning())
                                .child(
                                    icon("circle-alert", 12.)
                                        .text_color(vorkspace_ui::theme::warning()),
                                )
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
                        let bot = launcher.get(&id);
                        div()
                            .id(("rail-session", id as usize))
                            .debug_selector(move || format!("rail-session-{id}"))
                            .role(Role::Button)
                            .aria_label(s.title.clone())
                            .aria_selected(active)
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
                                        d.child(
                                            icon("pin", 11.)
                                                .flex_shrink_0()
                                                .text_color(content(0.45)),
                                        )
                                    })
                                    .children(self.work_item_badge(id, true, cx))
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
                                    .child(div().flex_1().min_w_0().truncate().child(s.project))
                                    .when_some(bot, |d, bot| {
                                        d.child(
                                            div()
                                                .debug_selector(move || {
                                                    format!("rail-session-bot-{id}")
                                                })
                                                .flex_shrink_0()
                                                .max_w(px(96.))
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .child(crate::avatar::bot_avatar(
                                                    bot, "idle", 12., true,
                                                ))
                                                .child(
                                                    div().min_w_0().truncate().child(bot.label()),
                                                ),
                                        )
                                    }),
                            )
                            .when(self.deleting.contains(&id), |d| d.opacity(0.5))
                            .relative()
                            .child(anchor.probe())
                            .on_hover(cx.listener(move |this, hovered, window, cx| {
                                this.hover_session(id, anchor.clone(), *hovered, true, window, cx);
                                this.invalidate_chrome(cx);
                            }))
                            .on_click(cx.listener(
                                move |this, event: &gpui::ClickEvent, window, cx| {
                                    if std::mem::take(&mut this.skip_session_click) {
                                        return;
                                    }
                                    this.select_sidebar_session(id, event.modifiers(), window, cx)
                                },
                            ))
                            // A card dragged onto a pane edge opens there.
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                                    this.skip_session_click = false;
                                    this.tab_drag = None;
                                    this.pane_drag = Some(PaneDrag {
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
                                    let anchor = Anchor::Point(event.position);
                                    this.context_selection = !this.selected_sessions.contains(&id);
                                    if this.context_selection {
                                        this.selected_sessions.clear();
                                        this.selected_sessions.insert(id);
                                    }
                                    this.open_menu(MenuTarget::Session(id), anchor, window, cx);
                                }),
                            )
                            .into_any_element()
                    })),
            )
            .children(disclosure)
            .into_any_element()
    }

    pub(super) fn vorkspace_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.vorkspace_dialog.as_ref()?;
        let editing = dialog.editing.clone();
        let def = self
            .workspace
            .vorkspaces
            .iter()
            .find(|v| v.id == editing)?
            .clone();
        let size = window.viewport_size();
        let width = 720f32.min(size.width.as_f32() - 24.);
        let height = 560f32.min(size.height.as_f32() - 32.);
        let weak = cx.weak_entity();
        let only = self.workspace.vorkspaces.len() <= 1;
        let look = self.vorkspace_look(&def, cx);
        let open_project = dialog.project.clone();
        let project_page = open_project.as_ref().map(|path| {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .mb_1()
                        .child(self.project_mark(path, 18.))
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.project_label(path)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child(crate::panel_kit::home_relative(std::path::Path::new(path))),
                )
                .child(
                    div()
                        .mt_5()
                        .mb_2()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.55))
                        .child("Codebase"),
                )
                .child(self.project_codebase_section(path, cx))
                .child(
                    div()
                        .mt_5()
                        .mb_2()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.55))
                        .child("Notifications"),
                )
                .child(self.project_notifications_section(path, cx))
                .into_any_element()
        });
        let mut nav: Vec<AnyElement> = Vec::new();
        for v in &self.workspace.vorkspaces {
            let id = v.id.clone();
            let current = v.id == editing;
            let selected = current && open_project.is_none();
            nav.push(
                div()
                    .id(SharedString::from(format!("vorkspace-row-{}", v.id)))
                    .debug_selector({
                        let id = v.id.clone();
                        move || format!("vorkspace-row-{id}")
                    })
                    .role(Role::Tab)
                    .aria_selected(selected)
                    .aria_label(v.name.clone())
                    .map(vorkspace_ui::theme::tab_reachable)
                    .h(px(34.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(radius::CONTROL))
                    .text_size(px(13.))
                    .text_color(content(if selected { 1. } else { 0.7 }))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(content(0.08)))
                    .when(!selected, |d| {
                        d.hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
                    })
                    .child(vorkspace_mark(v, 18.))
                    .child(div().flex_1().min_w_0().truncate().child(v.name.clone()))
                    .when(v.id == self.workspace.active_vorkspace, |d| {
                        d.child(icon("check-2", 12.).text_color(content(0.45)))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this
                            .vorkspace_dialog
                            .as_ref()
                            .is_some_and(|d| d.editing == id)
                        {
                            if let Some(d) = this.vorkspace_dialog.as_mut() {
                                d.project = None;
                                d.confirm_delete = false;
                            }
                            cx.notify();
                        } else {
                            this.open_vorkspace_dialog(Some(id.clone()), window, cx);
                        }
                    }))
                    .into_any_element(),
            );
            if !current {
                continue;
            }
            // Each of the vorkspace's projects has its own settings tab.
            for path in v
                .projects
                .iter()
                .filter(|p| self.workspace.is_project_available(p))
            {
                let selected = open_project.as_deref() == Some(path.as_str());
                let target = path.clone();
                let key = path_key(path);
                nav.push(
                    div()
                        .id(SharedString::from(format!("vorkspace-project-tab-{key}")))
                        .debug_selector(move || format!("vorkspace-project-tab-{key}"))
                        .role(Role::Tab)
                        .aria_selected(selected)
                        .aria_label(format!("{} settings", self.project_label(path)))
                        .map(vorkspace_ui::theme::tab_reachable)
                        .h(px(30.))
                        .pl(px(20.))
                        .pr(px(8.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(radius::CONTROL))
                        .text_size(px(12.5))
                        .text_color(content(if selected { 1. } else { 0.65 }))
                        .cursor_pointer()
                        .when(selected, |d| d.bg(content(0.08)))
                        .when(!selected, |d| {
                            d.hover(|s| s.bg(vorkspace_ui::fluid::hover(0.05)))
                        })
                        .child(self.project_mark(path, 14.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(self.project_label(path)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(d) = this.vorkspace_dialog.as_mut() {
                                d.project = Some(target.clone());
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        let section = |label: &'static str| {
            div()
                .mt_5()
                .mb_2()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(content(0.55))
                .child(label)
        };
        let projects = def
            .projects
            .clone()
            .into_iter()
            .filter(|p| self.workspace.is_project_available(p))
            .map(|path| {
                let remove = path.clone();
                let editing = editing.clone();
                let key = path_key(&path);
                div()
                    .id(SharedString::from(format!("vorkspace-project-{key}")))
                    .h(px(40.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(radius::CONTROL))
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.04)))
                    .child(self.project_mark(&path, 16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .child(self.project_label(&path)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(content(0.45))
                                    .child(crate::panel_kit::home_relative(std::path::Path::new(
                                        &path,
                                    ))),
                            ),
                    )
                    .child(
                        icon_button(
                            "vorkspace-remove-project",
                            "Remove from this vorkspace",
                            "close",
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.workspace
                                    .remove_project_from_vorkspace(&editing, &remove);
                                if editing == this.workspace.active_vorkspace
                                    && path_key(&remove) == path_key(&this.workspace.project)
                                {
                                    let landing = this.workspace.vorkspace_landing(&editing);
                                    this.open_project_from_user(landing, window, cx);
                                } else {
                                    this.persist(cx);
                                    cx.notify();
                                }
                            },
                        )),
                    )
            });
        // Folders already known elsewhere can join with one click.
        let addable: Vec<String> = self
            .workspace
            .projects
            .iter()
            .filter(|p| {
                self.workspace.is_project_available(p)
                    && !def.projects.iter().any(|q| path_key(q) == path_key(p))
            })
            .cloned()
            .collect();
        let chip = |id: SharedString, label: String, lead: AnyElement| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(format!("Add {label}"))
                .map(vorkspace_ui::theme::tab_reachable)
                .h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_full()
                .border_1()
                .border_color(content(0.12))
                .text_size(px(12.))
                .text_color(content(0.75))
                .cursor_pointer()
                .hover(|s| {
                    s.bg(vorkspace_ui::fluid::hover(0.06))
                        .text_color(content(1.))
                })
                .child(icon("plus", 12.))
                .child(lead)
                .child(label)
        };
        let add_projects = addable.into_iter().map(|path| {
            let editing = editing.clone();
            let add = path.clone();
            chip(
                SharedString::from(format!("vorkspace-add-project-{}", path_key(&path))),
                self.project_label(&path),
                self.project_mark(&path, 14.),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.workspace.add_project_to_vorkspace(&editing, &add);
                this.persist(cx);
                cx.notify();
            }))
        });
        let bots = self.vorkspace_bots(cx);
        let (members, others): (Vec<_>, Vec<_>) =
            bots.into_iter().partition(|b| def.bots.contains(&b.id));
        let no_bots = members.is_empty() && others.is_empty();
        let member_rows = members.into_iter().map(|bot| {
            let editing = editing.clone();
            let id = bot.id.clone();
            div()
                .id(SharedString::from(format!("vorkspace-bot-{}", bot.id)))
                .h(px(36.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(radius::CONTROL))
                .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.04)))
                .child(crate::avatar::bot_avatar(&bot, "idle", 18., true))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .child(bot.label()),
                )
                .child(
                    icon_button(
                        "vorkspace-remove-bot",
                        "Remove from this vorkspace",
                        "close",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(v) = this.workspace.vorkspace_mut(&editing) {
                            v.bots.retain(|b| *b != id);
                        }
                        this.reconcile_after_bot_move(cx);
                    })),
                )
        });
        let add_bots = others.into_iter().map(|bot| {
            let editing = editing.clone();
            let id = bot.id.clone();
            chip(
                SharedString::from(format!("vorkspace-add-bot-{}", bot.id)),
                bot.label(),
                crate::avatar::bot_avatar(&bot, "idle", 14., true),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.workspace.move_bot_to_vorkspace(&editing, &id);
                this.reconcile_after_bot_move(cx);
            }))
        });
        let confirm = dialog.confirm_delete;
        let delete_id = editing.clone();
        let switch_id = editing.clone();
        let is_active = editing == self.workspace.active_vorkspace;
        let body = div()
            .id("vorkspace-dialog-body")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("vorkspace-dialog-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .pt_4()
                    .pb_4()
                    .children(project_page)
                    .when(open_project.is_none(), |d| {
                        d.child(section("Name").mt_0())
                    .child(crate::controls::page::field_frame().child(dialog.name.clone()))
                    .child(section("Look"))
                    .child(look)
                    .child(section("Folders"))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .mb_2()
                            .child(
                                "A folder can be in several vorkspaces; its sessions show in each.",
                            ),
                    )
                    .children(projects)
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                chip(
                                    "vorkspace-add-folder".into(),
                                    "Add folder…".into(),
                                    div().into_any_element(),
                                )
                                .debug_selector(|| "vorkspace-add-folder".into())
                                .on_click(cx.listener(|this, _, _, cx| this.open_project(cx))),
                            )
                            .children(add_projects),
                    )
                    .child(section("Bots"))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .mb_2()
                            .child(if no_bots {
                                "A Bot created while this vorkspace is open belongs to it."
                            } else {
                                "A Bot belongs to one vorkspace; adding one here moves it."
                            }),
                    )
                    .children(member_rows)
                    .child(div().mt_2().flex().flex_wrap().gap_2().children(add_bots))
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_5()
                    .py_3()
                    .border_t_1()
                    .border_color(content(0.06))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        crate::controls::page::secondary(
                            "vorkspace-delete",
                            "Delete vorkspace",
                            true,
                            only,
                        )
                        .debug_selector(|| "vorkspace-delete".into())
                        .child(if confirm {
                            "Confirm delete"
                        } else {
                            "Delete vorkspace"
                        })
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if only {
                                    return;
                                }
                                let Some(d) = this.vorkspace_dialog.as_mut() else {
                                    return;
                                };
                                if !d.confirm_delete {
                                    d.confirm_delete = true;
                                    cx.notify();
                                    return;
                                }
                                let was_active = delete_id == this.workspace.active_vorkspace;
                                if this.workspace.delete_vorkspace(&delete_id) {
                                    if was_active {
                                        let landing = this.workspace.vorkspace_landing(
                                            &this.workspace.active_vorkspace.clone(),
                                        );
                                        this.open_project_from_user(landing, window, cx);
                                    }
                                    let next = this.workspace.active_vorkspace.clone();
                                    this.reconcile_vorkspace_bots(cx);
                                    this.persist(cx);
                                    this.open_vorkspace_dialog(Some(next), window, cx);
                                }
                            },
                        )),
                    )
                    .child(div().flex_1())
                    .when(!is_active, |d| {
                        d.child(
                            crate::controls::page::secondary(
                                "vorkspace-switch-here",
                                "Switch to this vorkspace",
                                false,
                                false,
                            )
                            .child("Switch to it")
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.switch_vorkspace(&switch_id, window, cx);
                                    this.open_vorkspace_dialog(Some(switch_id.clone()), window, cx);
                                },
                            )),
                        )
                    })
                    .child(
                        crate::controls::page::primary("vorkspace-done", "Done", false)
                            .debug_selector(|| "vorkspace-done".into())
                            .child("Done")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_vorkspace_dialog(window, cx)
                            })),
                    ),
            );
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(0.), px(0.)))
                            .child(
                                div()
                                    .w(size.width)
                                    .h(size.height)
                                    .bg(vorkspace_ui::fluid::scrim(0.4))
                                    .occlude(),
                            ),
                    )
                    .with_priority(59),
                )
                .child(
                    popover(
                        "vorkspace-modal",
                        Anchor::Point(gpui::point(
                            px((size.width.as_f32() - width) / 2.),
                            px((size.height.as_f32() - height) / 2.),
                        )),
                    )
                    .modal_surface(true)
                    .without_animation()
                    .priority(60)
                    .width(width)
                    .height(height)
                    .gap(0.)
                    .on_dismiss(move |window, cx| {
                        let _ = weak.update(cx, |this, cx| this.close_vorkspace_dialog(window, cx));
                    })
                    .child(
                        div()
                            .id("vorkspace-dialog")
                            .debug_selector(|| "vorkspace-dialog".into())
                            .role(Role::Dialog)
                            .aria_label("Vorkspaces")
                            .track_focus(&dialog.focus)
                            .on_key_down(cx.listener(
                                |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if event.keystroke.key == "escape" {
                                        this.close_vorkspace_dialog(window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .size_full()
                            .flex()
                            .child(
                                div()
                                    .id("vorkspace-dialog-nav")
                                    .role(Role::TabList)
                                    .aria_label("Vorkspaces")
                                    .w(px(200.))
                                    .flex_shrink_0()
                                    .h_full()
                                    .border_r_1()
                                    .border_color(content(0.04))
                                    .p_2()
                                    .flex()
                                    .flex_col()
                                    .gap_px()
                                    .child(
                                        div()
                                            .px_2()
                                            .pt_1()
                                            .pb_2()
                                            .text_size(px(12.))
                                            .text_color(content(0.5))
                                            .child("Vorkspaces"),
                                    )
                                    .children(nav)
                                    .child(div().flex_1())
                                    .child(
                                        crate::controls::page::action(
                                            "vorkspace-new",
                                            "New vorkspace",
                                            true,
                                        )
                                        .debug_selector(|| "vorkspace-new".into())
                                        .child(icon("plus", 12.))
                                        .child("New vorkspace")
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                let n = this.workspace.vorkspaces.len() + 1;
                                                let id = this
                                                    .workspace
                                                    .create_vorkspace(&format!("Vorkspace {n}"));
                                                this.persist(cx);
                                                this.open_vorkspace_dialog(Some(id), window, cx);
                                            }),
                                        ),
                                    ),
                            )
                            .child(body),
                    ),
                )
                .into_any_element(),
        )
    }

    /// The vorkspace's own customization: its color and the chat background
    /// every session in it shows. Projects carry none of their own.
    fn vorkspace_look(
        &self,
        def: &crate::workspace::VorkspaceDef,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let swatches = vorkspace_core::project_appearance::COLORS
            .iter()
            .skip(1)
            .copied()
            .map(|color| {
                let selected = def.color == Some(color);
                let editing = def.id.clone();
                div()
                    .id(SharedString::from(format!("vorkspace-color-{color:06x}")))
                    .role(Role::Button)
                    .aria_label(format!("Color #{color:06x}"))
                    .aria_selected(selected)
                    .map(vorkspace_ui::theme::tab_reachable)
                    .size(px(26.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(selected, |d| d.border_2().border_color(content(0.7)))
                    .child(crate::avatar::color_swatch_circle(color, None, 18.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(v) = this.workspace.vorkspace_mut(&editing) {
                            v.color = (v.color != Some(color)).then_some(color);
                        }
                        this.invalidate_chrome(cx);
                        this.persist(cx);
                        cx.notify();
                    }))
            });
        let editing = def.id.clone();
        let has_background = def.background.is_some();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().flex().flex_wrap().gap_1().children(swatches))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        crate::controls::page::secondary(
                            "vorkspace-background",
                            "Chat background",
                            false,
                            false,
                        )
                        .debug_selector(|| "vorkspace-background".into())
                        .child(if has_background {
                            "Change chat background…"
                        } else {
                            "Chat background…"
                        })
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_vorkspace_background(editing.clone(), window, cx)
                            },
                        )),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child("Shown behind every session in this vorkspace."),
                    ),
            )
            .into_any_element()
    }

    fn reconcile_after_bot_move(&mut self, cx: &mut Context<Self>) {
        let hidden = self.workspace.hidden_bots();
        if let Some(manager) = self.bots.clone() {
            manager.update(cx, |m, cx| {
                if m.active.as_ref().is_some_and(|id| hidden.contains(id)) {
                    m.close(cx);
                }
                m.vorkspace_hidden = hidden;
                cx.notify();
            });
        }
        self.persist(cx);
        cx.notify();
    }
}
