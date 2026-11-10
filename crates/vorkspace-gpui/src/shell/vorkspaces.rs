//! Vorkspaces in the window: the switcher atop the rail, the dialog that
//! edits a vorkspace's name, folders and Bots, and the rail's Sessions list.
//!
//! The model lives in `workspace::vorkspaces`; this file only presents it.
//! Switching never closes anything: it remembers the project the vorkspace
//! showed and lands on the target's last project, as a project click does.
use super::*;
use crate::popover::{MenuAction, MenuItem};
use gpui::Div;
use unicode_segmentation::UnicodeSegmentation;
use vorkspace_core::files::path_key;
use vorkspace_ui::radius;

pub(crate) struct VorkspaceDialog {
    pub(crate) editing: String,
    /// A project's own tab (Codebase, Notifications); none shows the
    /// vorkspace itself.
    pub(crate) project: Option<String>,
    pub(super) name: Entity<input::Input>,
    pub(super) icon_letters: Entity<input::Input>,
    pub(super) focus: FocusHandle,
    pub(super) confirm_delete: bool,
    _fields: [Subscription; 2],
}

/// Collapsed, the rail shows this many sessions: working and
/// approval-blocked ones first, then the latest.
const RAIL_SESSIONS: usize = 5;
const RAIL_CARD_HEIGHT: f32 = 46.;
const RAIL_ROW_HEIGHT: f32 = RAIL_CARD_HEIGHT + 1.;

/// A stable tint for a vorkspace's initial, from its id.
fn vorkspace_hue(id: &str) -> f32 {
    let hash = id
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    (hash % 360) as f32 / 360.
}

pub(crate) fn vorkspace_mark(def: &crate::workspace::VorkspaceDef, side: f32) -> Div {
    let label = def.icon_text();
    let multiple = label.graphemes(true).nth(1).is_some();
    let background: gpui::Hsla = match def.color {
        Some(color) => gpui::rgb(color).into(),
        None => gpui::hsla(vorkspace_hue(&def.id), 0.55, 0.5, 1.),
    };
    let rgb: gpui::Rgba = background.into();
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(rgb.r) + 0.7152 * linear(rgb.g) + 0.0722 * linear(rgb.b);
    div()
        .size(px(side))
        .flex_shrink_0()
        .rounded(px(vorkspace_ui::radius::CONTROL))
        .bg(background)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(side * if multiple { 0.42 } else { 0.55 }))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if luminance > 0.179 {
            gpui::black()
        } else {
            gpui::white()
        })
        .child(label)
}

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
    pub(super) fn switch_vorkspace(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.select_vorkspace(id, cx) {
            return;
        }
        let landing = self.workspace.vorkspace_landing(id);
        self.open_project_from_user(landing, window, cx);
        self.persist(cx);
        cx.notify();
    }

    fn select_vorkspace(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if id == self.workspace.active_vorkspace
            || !self.workspace.vorkspaces.iter().any(|v| v.id == id)
        {
            return false;
        }
        let from = self.workspace.active_vorkspace.clone();
        if self
            .workspace
            .vorkspace_has_project(&self.workspace.project)
        {
            self.workspace
                .vorkspace_project
                .insert(from, self.workspace.project.clone());
        }
        self.workspace.active_vorkspace = id.to_owned();
        self.rail_sessions_expanded = false;
        self.rail_search_open = false;
        self.query.clear();
        self.sidebar_search
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.selected_sessions.clear();
        self.selection_anchor = None;
        self.sidebar_target = None;
        self.configure_vorkspace_bots(cx);
        true
    }

    /// Cross-project history and search keep the current vorkspace when it
    /// shares the folder, otherwise they take the user to a vorkspace with it.
    pub(super) fn route_project(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        if path.is_empty() {
            return true;
        }
        let Some(id) = self
            .workspace
            .vorkspace_for_project(path)
            .map(str::to_owned)
        else {
            self.error = Some(if self.workspace.is_project_available(path) {
                "Add this project to a vorkspace before opening its conversations.".into()
            } else {
                "Restore this project before opening its conversations.".into()
            });
            cx.notify();
            return false;
        };
        if self.select_vorkspace(&id, cx) {
            self.workspace.project = path.to_owned();
            self.workspace.active = None;
        }
        true
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
        let id = id
            .filter(|id| self.workspace.vorkspaces.iter().any(|v| &v.id == id))
            .unwrap_or_else(|| self.workspace.active_vorkspace.clone());
        let (name, letters) = self
            .workspace
            .vorkspaces
            .iter()
            .find(|v| v.id == id)
            .map(|v| (v.name.clone(), v.icon_letters().to_owned()))
            .unwrap_or_default();
        let name = cx.new(|cx| {
            let mut field =
                input::Input::field("Vorkspace name", "Vorkspace name", content(0.35), cx);
            field.set_text(name, cx);
            field
        });
        let name_sub = cx.subscribe(&name, |this, field, _: &input::Changed, cx| {
            let Some(editing) = this
                .vorkspace_dialog
                .as_ref()
                .filter(|d| d.name == field)
                .map(|d| &d.editing)
            else {
                return;
            };
            let name = field.read(cx).content.trim().to_owned();
            if name.is_empty() {
                return;
            }
            if let Some(v) = this.workspace.vorkspace_mut(editing)
                && v.name != name
            {
                v.name = name;
                this.persist(cx);
                cx.notify();
            }
        });
        let icon_letters = cx.new(|cx| {
            let mut field = input::Input::field("Icon letters", "Auto", content(0.45), cx);
            field.set_centered(true);
            field.set_text(letters, cx);
            field
        });
        let icon_sub = cx.subscribe(&icon_letters, |this, field, _: &input::Changed, cx| {
            let input = field.read(cx);
            if input.is_composing() {
                return;
            }
            let Some(editing) = this
                .vorkspace_dialog
                .as_ref()
                .filter(|d| d.icon_letters == field)
                .map(|d| &d.editing)
            else {
                return;
            };
            let text = input.content.clone();
            let Some(v) = this.workspace.vorkspace_mut(editing) else {
                return;
            };
            let changed = v.set_icon_letters(&text);
            if text.as_ref() != v.icon_letters() {
                let canonical = v.icon_letters().to_owned();
                field.update(cx, |field, cx| field.set_text(canonical, cx));
            }
            if changed {
                this.persist(cx);
                cx.notify();
            }
        });
        let focus = cx.focus_handle();
        self.navigation_revision += 1;
        window.focus(&name.focus_handle(cx), cx);
        self.vorkspace_dialog = Some(VorkspaceDialog {
            editing: id,
            project: None,
            name,
            icon_letters,
            focus,
            confirm_delete: false,
            _fields: [name_sub, icon_sub],
        });
        cx.notify();
    }

    pub(super) fn close_vorkspace_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.vorkspace_dialog.take().is_some() {
            self.focus_workspace(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn management_modal_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.background_dialog
            .as_ref()
            .map(|background| background.view.focus_handle(cx))
            .or_else(|| {
                self.vorkspace_dialog
                    .as_ref()
                    .map(|dialog| dialog.focus.clone())
            })
    }

    fn vorkspace_bots(&self, cx: &App) -> Vec<vorkspace_core::bot::Bot> {
        self.app_core
            .as_ref()
            .and_then(|core| core.read(cx).bot_roster.read(cx).committed())
            .map(|roster| {
                roster
                    .bots
                    .iter()
                    .filter(|b| !b.is_minion())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn configure_vorkspace_bots(&self, cx: &mut Context<Self>) {
        let Some(manager) = self.bots.clone() else {
            return;
        };
        let hidden = self.workspace.hidden_bots();
        let active = self.workspace.active_vorkspace.clone();
        manager.update(cx, |m, _| m.set_vorkspace(active, hidden));
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

    fn rail_sessions(
        &self,
        launches: &HashMap<i64, &vorkspace_core::bot::Bot>,
        cx: &App,
    ) -> (Vec<i64>, usize) {
        let mut sessions = self.filtered_rail_sessions(launches, cx);
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
        self.rail_sessions(&self.session_launchers(cx), cx).0
    }

    pub(super) fn rail_sessions_min_height(&self) -> f32 {
        if self.rail_search_open || !self.query.is_empty() {
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
        let hovered = self.session_hover;
        self.session_row_bounds.retain(|id, _| Some(*id) == hovered);
        if self.rail_list_ids.as_slice() != sessions.as_slice() {
            let mut offset = self.rail_list.logical_scroll_top();
            let anchor = self.rail_list_ids.get(offset.item_ix);
            self.rail_list
                .reset_with_uniform_height(sessions.len(), px(RAIL_ROW_HEIGHT));
            if (offset.item_ix != 0 || offset.offset_in_item > px(0.))
                && let Some(ix) = anchor.and_then(|id| sessions.iter().position(|row| row == id))
            {
                offset.item_ix = ix;
                self.rail_list.scroll_to(offset);
            }
            self.rail_list_ids = std::rc::Rc::new(sessions);
        }
        let sessions = self.rail_list_ids.clone();
        let owner = cx.weak_entity();
        self.rail_list.set_scroll_handler(move |_, _, cx| {
            let _ = owner.update(cx, |this, cx| {
                this.sidebar_last_scroll = Some(vorkspace_ui::clock::Instant::now());
                this.close_preview(cx);
            });
        });
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
                    } else if filtering || !self.query.is_empty() {
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
                        gpui::list(self.rail_list.clone(), move |ix, _, cx| {
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
        if let Some(row) = self.rename_row(&super::session_groups::RenameTarget::Session(id), cx) {
            return row;
        }
        let active = self.selected_sessions.contains(&id)
            || (self.workspace.active == Some(id) && !self.bot_active(cx) && !self.app_view_open());
        let anchor = self.session_row_bounds.entry(id).or_default().clone();
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
                s.project,
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
                this.hover_session(id, anchor.clone(), *hovered, true, window, cx);
                this.invalidate_chrome(cx);
            }))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if std::mem::take(&mut this.skip_session_click) {
                        return;
                    }
                    this.select_sidebar_session(id, event.modifiers(), window, cx)
                }),
            )
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
        self.context_selection = !self.selected_sessions.contains(&id);
        if self.context_selection {
            self.selected_sessions.clear();
            self.selected_sessions.insert(id);
        }
        self.open_menu(MenuTarget::Session(id), anchor, window, cx);
        window.prevent_default();
        cx.stop_propagation();
    }

    pub(super) fn vorkspace_modal(
        &self,
        window: &mut Window,
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
        let look = self.vorkspace_look(&def, &dialog.icon_letters, cx);
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
        let nav_state = window.use_keyed_state("vorkspace-dialog-navigation", cx, |_, _| {
            (
                gpui::ScrollHandle::new(),
                std::cell::RefCell::<Option<FocusHandle>>::default(),
            )
        });
        let focused = window.focused(cx);
        let (scroll, previous_focus) = nav_state.read(cx);
        let focus_changed = previous_focus.replace(focused.clone()) != focused;
        let scroll = scroll.clone();
        let nav_focus = |key: SharedString, ix, window: &mut Window, cx: &mut App| {
            let handle = window
                .use_keyed_state(key, cx, |_, cx| cx.focus_handle().tab_stop(true))
                .read(cx)
                .clone();
            if focus_changed && focused.as_ref() == Some(&handle) {
                scroll.scroll_to_item(ix);
            }
            handle
        };
        let mut nav: Vec<AnyElement> = Vec::new();
        for v in &self.workspace.vorkspaces {
            let id = v.id.clone();
            let current = v.id == editing;
            let selected = current && open_project.is_none();
            let focus = nav_focus(format!("vorkspace-nav-{id}").into(), nav.len(), window, cx);
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
                    .track_focus(&focus)
                    .h(px(34.))
                    .flex_shrink_0()
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
                let focus = nav_focus(
                    format!("vorkspace-nav-project-{}-{key}", v.id).into(),
                    nav.len(),
                    window,
                    cx,
                );
                nav.push(
                    div()
                        .id(SharedString::from(format!("vorkspace-project-tab-{key}")))
                        .debug_selector(move || format!("vorkspace-project-tab-{key}"))
                        .role(Role::Tab)
                        .aria_selected(selected)
                        .aria_label(format!("{} settings", self.project_label(path)))
                        .map(vorkspace_ui::theme::tab_reachable)
                        .track_focus(&focus)
                        .h(px(30.))
                        .flex_shrink_0()
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
                                d.confirm_delete = false;
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
                        .aria_label(format!(
                            "Remove {} from this vorkspace",
                            self.project_label(&path)
                        ))
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
            let debug_id = id.clone();
            div()
                .id(id)
                .debug_selector(move || debug_id.to_string())
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
            .on_click(cx.listener(move |this, _, window, cx| {
                let first = editing == this.workspace.active_vorkspace
                    && (this.workspace.project.is_empty()
                        || !this
                            .workspace
                            .vorkspace_projects()
                            .iter()
                            .any(|p| path_key(p) == path_key(&this.workspace.project)));
                this.workspace.add_project_to_vorkspace(&editing, &add);
                if first {
                    this.open_project_from_user(add.clone(), window, cx);
                } else {
                    this.persist(cx);
                    cx.notify();
                }
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
                    .aria_label(format!("Remove {} from this vorkspace", bot.label()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_bot_membership(&id, &editing, false, cx);
                        this.reconcile_after_bot_move(window, cx);
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
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_bot_membership(&id, &editing, true, cx);
                this.reconcile_after_bot_move(window, cx);
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
                    .when(open_project.is_none(), |d| {
                        d.child(
                            crate::controls::page::secondary(
                                "vorkspace-delete",
                                if confirm {
                                    "Confirm delete"
                                } else {
                                    "Delete vorkspace"
                                },
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
                                    if d.project.is_some() || d.editing != delete_id {
                                        return;
                                    }
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
                                        this.configure_vorkspace_bots(cx);
                                        this.persist(cx);
                                        this.open_vorkspace_dialog(Some(next), window, cx);
                                    }
                                },
                            )),
                        )
                    })
                    .child(div().flex_1())
                    .when(open_project.is_none() && !is_active, |d| {
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
                            .text_color(content(1.))
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
                                    .child(
                                        div()
                                            .id("vorkspace-dialog-nav")
                                            .debug_selector(|| "vorkspace-dialog-nav".into())
                                            .role(Role::TabList)
                                            .aria_label("Vorkspaces")
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .track_scroll(&scroll)
                                            .flex()
                                            .flex_col()
                                            .gap_px()
                                            .children(nav),
                                    )
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

    /// The vorkspace's mark and the chat background its sessions show.
    fn vorkspace_look(
        &self,
        def: &crate::workspace::VorkspaceDef,
        letters: &Entity<input::Input>,
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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.75))
                            .child("Icon letters"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(vorkspace_mark(def, 26.))
                            .child(
                                crate::controls::page::field_frame()
                                    .w(px(64.))
                                    .flex_shrink_0()
                                    .child(letters.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .text_color(content(0.55))
                                    .child("Up to two letters. Clear to use the name’s initial."),
                            ),
                    ),
            )
            .child(div().flex().flex_wrap().gap_1().children(swatches))
            .child(
                div()
                    .flex()
                    .flex_wrap()
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

    fn reconcile_after_bot_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sync_vorkspace_bot_visibility(cx) {
            if let Some(active) = self.workspace.active {
                self.open(active, window, cx);
            } else {
                self.ensure_project_session(window, cx);
            }
        }
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn sync_vorkspace_bot_visibility(&mut self, cx: &mut Context<Self>) -> bool {
        let hidden = self.workspace.hidden_bots();
        let active = self.workspace.active_vorkspace.clone();
        if let Some(manager) = self.bots.clone() {
            manager.update(cx, |m, cx| {
                if m.active.as_ref().is_some_and(|id| hidden.contains(id)) {
                    m.close(cx);
                }
                m.set_vorkspace(active, hidden);
                cx.notify();
            });
        }
        let Some(active) = self.workspace.active else {
            return false;
        };
        if !self
            .workspace
            .app_panes
            .get(&active)
            .is_some_and(|pane| match &pane.content {
                crate::workspace::AppPane::Bot(id)
                | crate::workspace::AppPane::BotDocument { bot: id, .. } => {
                    self.workspace.bot_hidden_in_vorkspace(id)
                }
                _ => false,
            })
        {
            return false;
        }
        let next = self
            .workspace
            .active_tab()
            .and_then(|tab| self.available_tab_focus(tab, cx))
            .or_else(|| {
                self.visible_tabs(cx)
                    .iter()
                    .rev()
                    .find_map(|tab| self.available_tab_focus(tab.session()?, cx))
            });
        self.workspace.active = None;
        if let Some(next) = next {
            self.workspace.open(next);
        }
        self.review_focused = false;
        self.update_chat_visibility(cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn history_switches_vorkspace_with_the_conversation(cx: &mut TestAppContext) {
        let (window, a, b) = super::super::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                let main = shell.workspace.active_vorkspace.clone();
                let side = shell.workspace.create_vorkspace("Side");
                let project = "/work/another-project";
                shell
                    .workspace
                    .remove_project_from_vorkspace(&main, project);
                shell.workspace.add_project_to_vorkspace(&side, project);

                shell.open(a, window, cx);
                assert_eq!(shell.workspace.active_vorkspace, side);
                assert_eq!(shell.workspace.project, project);
                shell.open(b, window, cx);
                assert_eq!(shell.workspace.active_vorkspace, main);
                shell.visit(-1, window, cx);
                assert_eq!(shell.workspace.active_vorkspace, side);
                assert_eq!(shell.workspace.active, Some(a));
                shell.visit(1, window, cx);
                assert_eq!(shell.workspace.active_vorkspace, main);
                assert_eq!(shell.workspace.active, Some(b));
            })
            .unwrap();
    }

    #[gpui::test]
    fn managing_vorkspaces_keeps_new_session_shortcut_out_of_the_chat(cx: &mut TestAppContext) {
        cx.update(super::super::bind_keys);
        let (window, _, b) = super::super::tests::window(cx);
        let before = window
            .update(cx, |shell, w, cx| {
                shell.open(b, w, cx);
                shell.open_vorkspace_dialog(None, w, cx);
                shell.order.len()
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-n");
        visual.run_until_parked();
        window
            .update(visual, |shell, w, cx| {
                assert_eq!(shell.order.len(), before);
                assert_eq!(shell.workspace.active, Some(b));
                assert!(
                    shell
                        .management_modal_focus(cx)
                        .unwrap()
                        .contains_focused(w, cx)
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn managing_many_folders_scrolls_the_focused_project_into_view(cx: &mut TestAppContext) {
        cx.update(super::super::bind_keys);
        let (window, _, _) = super::super::tests::window(cx);
        window
            .update(cx, |shell, w, cx| {
                let editing = shell.workspace.active_vorkspace.clone();
                for n in 0..40 {
                    shell
                        .workspace
                        .add_project_to_vorkspace(&editing, &format!("/work/folder-{n}"));
                }
                shell.open_vorkspace_dialog(None, w, cx);
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(gpui::size(px(700.), px(480.)));
        visual.run_until_parked();
        // Move from the name field backwards past the fixed New button.
        visual.simulate_keystrokes("shift-tab shift-tab");
        visual.run_until_parked();
        let nav = visual.debug_bounds("vorkspace-dialog-nav").unwrap();
        let last = visual
            .debug_bounds("vorkspace-project-tab-/work/folder-39")
            .unwrap();
        let add = visual.debug_bounds("vorkspace-new").unwrap();
        assert!(
            last.top() >= nav.top() && last.bottom() <= nav.bottom(),
            "{last:?} outside {nav:?}"
        );
        assert!(add.top() >= nav.bottom());
        visual.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
            is_held: false,
            prefer_character_input: false,
        });
        visual.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        window
            .update(visual, |shell, _, _| {
                assert_eq!(
                    shell.vorkspace_dialog.as_ref().unwrap().project.as_deref(),
                    Some("/work/folder-39")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn adding_first_folder_chip_to_active_empty_vorkspace_enters_that_project(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = super::super::tests::window(cx);
        let second_project = "/work/second".to_string();
        window
            .update(cx, |shell, window, cx| {
                shell.workspace.add_project(&second_project);
                let empty_id = shell.workspace.create_vorkspace("Empty");
                shell.switch_vorkspace(&empty_id, window, cx);
                assert_eq!(shell.workspace.active_vorkspace, empty_id);
                assert!(shell.workspace.project.is_empty());
                assert!(shell.workspace.vorkspace_projects().is_empty());

                shell.open_vorkspace_dialog(Some(empty_id.clone()), window, cx);
            })
            .unwrap();

        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let chip_id: &'static str =
            format!("vorkspace-add-project-{}", path_key(&second_project)).leak();
        let chip_bounds = visual
            .debug_bounds(chip_id)
            .expect("add project chip rendered");
        visual.simulate_click(chip_bounds.center(), Default::default());
        visual.run_until_parked();

        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.project, second_project);
                assert_eq!(shell.workspace.vorkspace_projects(), [second_project]);
            })
            .unwrap();
    }
}
