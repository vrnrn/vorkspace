use vorkspace_ui::radius;
// Project terminal dock, tab actions and resize interactions.
use super::*;
use crate::{
    popover::{Anchor, MenuAction, MenuItem},
    terminal::Terminal,
    workspace::{DockSide, TerminalDock},
};
use gpui::{
    AnyElement, App, Bounds, Context, Entity, EntityId, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Role, SharedString, Window, div, point, prelude::*, px,
};
use std::collections::HashMap;
use vorkspace_ui::clock::Instant;
use vorkspace_ui::theme::*;

/// useAnimatedReorder's drag of one dock tab along the strip.
pub(super) struct DockTabDrag {
    from: usize,
    to: usize,
    origin: Pixels,
    offset: Pixels,
    active: bool,
    bounds: Vec<Bounds<Pixels>>,
    order: Vec<EntityId>,
}

impl DockTabDrag {
    fn matches(&self, live: &[Entity<Terminal>]) -> bool {
        self.order
            .iter()
            .copied()
            .eq(live.iter().map(Entity::entity_id))
    }
}

/// Where a tab is drawn now, easing toward `target` (the title strip's
/// `eased_shift`).
fn eased_shift(
    shifts: &mut HashMap<EntityId, TabShift>,
    id: EntityId,
    target: f32,
    follow: bool,
) -> (f32, bool) {
    let now = Instant::now();
    if follow {
        shifts.remove(&id);
        return (target, false);
    }
    let current = shifts.get(&id).map(|s| s.at(now));
    if current.is_none_or(|_| shifts[&id].to != target) {
        let from = current.map_or(0., |(value, _)| value);
        if from == target {
            shifts.remove(&id);
            return (target, false);
        }
        shifts.insert(
            id,
            TabShift {
                from,
                to: target,
                start: now,
            },
        );
    }
    let (value, done) = shifts[&id].at(now);
    if done && target == 0. {
        shifts.remove(&id);
    }
    (value, !done)
}

impl Shell {
    fn live_terminals(&self) -> Vec<Entity<Terminal>> {
        self.dock
            .terminals
            .get(&self.workspace.project)
            .cloned()
            .unwrap_or_default()
    }
    fn dock_terminal_index(&self, id: EntityId) -> Option<usize> {
        self.dock
            .terminals
            .get(&self.workspace.project)?
            .iter()
            .position(|t| t.entity_id() == id)
    }

    pub(super) fn terminal_tab_menu(&self, id: EntityId) -> Vec<MenuItem> {
        let live = self.dock_terminal_index(id).is_some();
        let multiple = self
            .dock
            .terminals
            .get(&self.workspace.project)
            .is_some_and(|t| t.len() > 1);
        vec![
            MenuAction::new("close", "Close").disabled(!live).into(),
            MenuAction::new("close-others", "Close Others")
                .disabled(!live || !multiple)
                .into(),
        ]
    }
    pub(super) fn run_terminal_tab_menu(
        &mut self,
        id: EntityId,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keep = match action {
            "close" => None,
            "close-others" => Some(id),
            _ => return,
        };
        self.request_dock_close(id, keep, window, cx);
    }

    fn request_dock_close(
        &mut self,
        id: EntityId,
        keep: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dock_terminal_index(id).is_none() {
            return;
        }
        let project = self.workspace.project.clone();
        let live = self.live_terminals();
        let mut closing = Vec::new();
        let mut running = Vec::new();
        for terminal in &live {
            let terminal_id = terminal.entity_id();
            let selected = match keep {
                Some(keep) => terminal_id != keep,
                None => terminal_id == id,
            };
            if selected {
                closing.push(terminal_id);
                let terminal = terminal.read(cx);
                if let Some(process) = terminal.foreground_process() {
                    running.push((project_name(&terminal.cwd), process));
                }
            }
        }
        let message = match running.as_slice() {
            [] => None,
            [(label, process)] => Some(format!(
                "\"{process}\" is still running in {label}. Close this terminal anyway?"
            )),
            many => Some(format!(
                "These terminals are still running:\n{}\n\nClose them anyway?",
                many.iter()
                    .map(|(label, process)| format!("• {label} ({process})"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )),
        };
        let Some(message) = message else {
            self.finish_dock_close(&project, &closing, keep, window, cx);
            return;
        };
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &message,
            None,
            &["Close", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_dock_close(&project, &closing, keep, window, cx)
                });
            }
        })
        .detach();
    }
    fn finish_dock_close(
        &mut self,
        project: &str,
        closing: &[EntityId],
        keep: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.workspace.project == project;
        let had_focus = current && self.dock_focused(window, cx).is_some();
        let (Some(live), Some(dock)) = (
            self.dock.terminals.get_mut(project),
            self.workspace.docks.get_mut(project),
        ) else {
            return;
        };
        if live.len() != dock.terminals.len()
            || keep.is_some_and(|id| live.iter().all(|t| t.entity_id() != id))
        {
            return;
        }
        let mut changed = false;
        for ix in (0..live.len()).rev() {
            if closing.contains(&live[ix].entity_id()) {
                live.remove(ix);
                dock.close(ix);
                changed = true;
            }
        }
        if !changed {
            return;
        }
        if let Some(ix) = keep.and_then(|id| live.iter().position(|t| t.entity_id() == id)) {
            dock.active = ix;
        }
        let empty = live.is_empty();
        if empty {
            dock.open = false;
        }
        if current {
            self.dock.drag = None;
            self.dock.shifts.clear();
            if empty {
                self.slide_dock(false);
                if had_focus {
                    self.focus_active(window, cx);
                }
            } else if had_focus {
                self.focus_terminal(window, cx);
            }
        }
        self.persist(cx);
        cx.notify();
    }
    /// SurfaceTabs selects a tab as it is pressed, then lets it be dragged.
    fn start_dock_drag(
        &mut self,
        id: EntityId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dock.skip_click = false;
        self.dock.drag = None;
        let Some(ix) = self.dock_terminal_index(id) else {
            return;
        };
        self.select_terminal(ix, window, cx);
        let live = self.live_terminals();
        let count = live.len();
        if count < 2 {
            return;
        }
        let bounds: Vec<_> = (0..count)
            .filter_map(|ix| self.dock.tab_scroll.bounds_for_item(ix))
            .collect();
        if bounds.len() != count {
            return;
        }
        self.dock.drag = Some(DockTabDrag {
            from: ix,
            to: ix,
            origin: event.position.x,
            offset: px(0.),
            active: false,
            bounds,
            order: live.iter().map(Entity::entity_id).collect(),
        });
    }
    pub(super) fn move_dock_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let live = self
            .dock
            .terminals
            .get(&self.workspace.project)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let Some(drag) = self.dock.drag.as_mut() else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) || !drag.matches(live) {
            self.dock.drag = None;
            cx.notify();
            return;
        }
        let delta = event.position.x - drag.origin;
        if delta.abs() < px(4.) && !drag.active {
            return;
        }
        drag.active = true;
        self.dock.skip_click = true;
        let start = drag.bounds[drag.from].left();
        drag.offset = delta.clamp(
            drag.bounds[0].left() - start,
            drag.bounds.last().unwrap().left() - start,
        );
        let at = start + drag.offset;
        drag.to = drag
            .bounds
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (a.left() - at)
                    .abs()
                    .partial_cmp(&(b.left() - at).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(drag.from, |(ix, _)| ix);
        cx.notify();
    }
    pub(super) fn finish_dock_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.dock.drag.take() else {
            return;
        };
        let live = self.live_terminals();
        if !drag.active || drag.from == drag.to || !drag.matches(&live) {
            cx.notify();
            return;
        }
        let project = self.workspace.project.clone();
        let now = Instant::now();
        // Each tab glides from where it is drawn into its new slot.
        let drawn: Vec<(EntityId, f32)> = live
            .iter()
            .enumerate()
            .map(|(ix, t)| {
                let id = t.entity_id();
                let shift = self.dock.shifts.get(&id).map_or(0., |s| s.at(now).0);
                let shift = if ix == drag.from {
                    drag.offset.as_f32()
                } else {
                    shift
                };
                (
                    id,
                    drag.bounds.get(ix).map_or(0., |b| b.left().as_f32()) + shift,
                )
            })
            .collect();
        if let Some(live) = self.dock.terminals.get_mut(&project) {
            let moved = live.remove(drag.from);
            live.insert(drag.to, moved);
        }
        if let Some(dock) = self.workspace.docks.get_mut(&project) {
            dock.reorder(drag.from, drag.to);
        }
        self.dock.shifts.clear();
        for (ix, terminal) in self.live_terminals().iter().enumerate() {
            let id = terminal.entity_id();
            let (Some((_, left)), Some(slot)) =
                (drawn.iter().find(|(t, _)| *t == id), drag.bounds.get(ix))
            else {
                continue;
            };
            let from = left - slot.left().as_f32();
            if from.abs() > 0.5 {
                self.dock.shifts.insert(
                    id,
                    TabShift {
                        from,
                        to: 0.,
                        start: now,
                    },
                );
            }
        }
        self.persist(cx);
        cx.notify();
    }
    fn dock_tab_shift(&self, ix: usize) -> Pixels {
        let Some(d) = self.dock.drag.as_ref().filter(|d| d.active) else {
            return px(0.);
        };
        if ix == d.from {
            return d.offset;
        }
        if d.from < d.to && ix > d.from && ix <= d.to {
            return d.bounds[ix - 1].left() - d.bounds[ix].left();
        }
        if d.from > d.to && ix >= d.to && ix < d.from {
            return d.bounds[ix + 1].left() - d.bounds[ix].left();
        }
        px(0.)
    }
    /// SurfaceTabs for the dock's terminals, then New, Move and Hide.
    pub(super) fn dock_header(
        &mut self,
        dock: &TerminalDock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let live = self.live_terminals();
        if self
            .dock
            .drag
            .as_ref()
            .is_some_and(|drag| !drag.matches(&live))
        {
            self.dock.drag = None;
        }
        let hide = match dock.side {
            DockSide::Bottom => "chevron-down",
            DockSide::Top => "chevron-up",
            DockSide::Left => "chevron-left",
            DockSide::Right => "chevron-right",
        };
        let side = match dock.side {
            DockSide::Bottom => "panel-bottom",
            DockSide::Top => "panel-top",
            DockSide::Left => "panel-left",
            DockSide::Right => "panel-right",
        };
        let dragged = self.dock.drag.as_ref().filter(|d| d.active).map(|d| d.from);
        let mut moving = false;
        let shifts: Vec<f32> = live
            .iter()
            .enumerate()
            .map(|(ix, terminal)| {
                let target = self.dock_tab_shift(ix).as_f32();
                let (value, active) = eased_shift(
                    &mut self.dock.shifts,
                    terminal.entity_id(),
                    target,
                    dragged == Some(ix),
                );
                moving |= active;
                value
            })
            .collect();
        let animate = self.workspace.preferences.tab_animations && dragged.is_none();
        let keyed = live
            .iter()
            .map(|t| (t.entity_id(), SharedString::from(t.read(cx).title.clone())))
            .collect();
        let (slots, opening) =
            self.dock
                .tab_motion
                .sync(keyed, animate, vorkspace_ui::clock::Instant::now());
        if moving || opening {
            window.request_animation_frame();
        }
        let move_anchor = self.dock.move_terminal_anchor.clone();
        div()
            .h(px(36.))
            .flex_shrink_0()
            .min_w_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(crate::panel_kit::stroke())
            .child(
                div()
                    .id("terminal-tabs")
                    .role(Role::TabList)
                    .aria_label("Terminals")
                    .track_scroll(&self.dock.tab_scroll)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .pl(px(6.))
                    .pr(px(10.))
                    // Every tab fits, sharing the strip however many there are.
                    .overflow_hidden()
                    .children({
                        let mut tabs: Vec<Option<AnyElement>> = live
                            .iter()
                            .enumerate()
                            .map(|(ix, terminal)| {
                                let id = terminal.entity_id();
                                let t = terminal.read(cx);
                                let label: SharedString = t.title.clone().into();
                                let description =
                                    SharedString::from(format!("{} — {}", t.title, t.cwd));
                                let active = ix == dock.active;
                                let held = dragged == Some(ix);
                                let hovered = self.dock.hover == Some(ix) && dragged.is_none();
                                div()
                                    .id(("terminal-tab", ix))
                                    .debug_selector(move || format!("terminal-tab-{ix}"))
                                    .relative()
                                    .left(px(shifts[ix]))
                                    .when(held, |d| d.cursor_grabbing())
                                    .h_full()
                                    .w(px(224.))
                                    .min_w_0()
                                    .flex_shrink(1.)
                                    .flex()
                                    .items_center()
                                    .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                                        let next = if *hovering {
                                            Some(ix)
                                        } else if this.dock.hover == Some(ix) {
                                            None
                                        } else {
                                            this.dock.hover
                                        };
                                        if next != this.dock.hover {
                                            this.dock.hover = next;
                                            cx.notify();
                                        }
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                this.start_dock_drag(id, event, window, cx)
                                            },
                                        ),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Middle,
                                        cx.listener(move |this, _, window, cx| {
                                            this.request_dock_close(id, None, window, cx)
                                        }),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                cx.stop_propagation();
                                                let Some(ix) = this.dock_terminal_index(id) else {
                                                    return;
                                                };
                                                this.select_terminal(ix, window, cx);
                                                this.open_menu(
                                                    MenuTarget::TerminalTab(id),
                                                    Anchor::Point(event.position),
                                                    window,
                                                    cx,
                                                );
                                            },
                                        ),
                                    )
                                    .child(
                                        div()
                                            .id(("terminal-tab-button", ix))
                                            .role(Role::Tab)
                                            .aria_label(label.clone())
                                            .aria_description(description)
                                            .aria_selected(active)
                                            .h(px(TAB_PLATE_HEIGHT))
                                            .min_w_0()
                                            .flex_1()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .px_2()
                                            .pr(px(28.))
                                            .rounded(px(radius::CONTROL))
                                            .text_size(px(13.))
                                            .map(|d| {
                                                if active {
                                                    d.bg(crate::panel_kit::selection())
                                                        .text_color(content(1.))
                                                } else if hovered {
                                                    d.bg(content(0.05)).text_color(content(1.))
                                                } else {
                                                    d.text_color(content(0.5))
                                                }
                                            })
                                            .child(icon("terminal", 14.).text_color(
                                                if active || hovered {
                                                    content(1.)
                                                } else {
                                                    content(0.5)
                                                },
                                            ))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .child(label.clone()),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if std::mem::take(&mut this.dock.skip_click) {
                                                    return;
                                                }
                                                if let Some(ix) = this.dock_terminal_index(id) {
                                                    this.select_terminal(ix, window, cx);
                                                }
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id(("terminal-tab-close", ix))
                                            .role(Role::Button)
                                            .map(vorkspace_ui::theme::tab_reachable)
                                            .aria_label(SharedString::from(format!(
                                                "Close {label}"
                                            )))
                                            .group("terminal-tab-close")
                                            .absolute()
                                            .right_1()
                                            .top(px(8.))
                                            .size(px(20.))
                                            .rounded(px(radius::SMALL))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .opacity(if (active || hovered) && !held {
                                                1.
                                            } else {
                                                0.
                                            })
                                            .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.1)))
                                            .child(
                                                icon("x", 12.)
                                                    .text_color(content(0.5))
                                                    .group_hover("terminal-tab-close", |s| {
                                                        s.text_color(content(1.))
                                                    }),
                                            )
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.request_dock_close(id, None, window, cx);
                                            })),
                                    )
                                    .into_any_element()
                            })
                            .map(Some)
                            .collect();
                        let ids: Vec<_> = live.iter().map(|t| t.entity_id()).collect();
                        // TabWidthMotion: `w-56` eases in from nothing, or out to it.
                        let slot = |open: f32| {
                            div()
                                .h_full()
                                .w(px(224. * open))
                                .min_w_0()
                                .flex_shrink(1.)
                                .mr(px(-vorkspace_ui::tab_motion::GAP * (1. - open)))
                                .overflow_hidden()
                                .flex()
                        };
                        slots
                            .into_iter()
                            .filter_map(move |s| match s {
                                vorkspace_ui::tab_motion::Slot::Live { key, open } => {
                                    let ix = ids.iter().position(|id| *id == key)?;
                                    let tab = tabs.get_mut(ix)?.take()?;
                                    Some(if open >= 1. {
                                        tab
                                    } else {
                                        slot(open).child(tab).into_any_element()
                                    })
                                }
                                vorkspace_ui::tab_motion::Slot::Closing { ghost, open, .. } => {
                                    Some(
                                        slot(open)
                                            .child(
                                                div()
                                                    .w(px(224.))
                                                    .h_full()
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .h(px(TAB_PLATE_HEIGHT))
                                                            .w_full()
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(6.))
                                                            .px_2()
                                                            .rounded(px(radius::CONTROL))
                                                            .text_size(px(13.))
                                                            .text_color(content(0.5))
                                                            .child(
                                                                icon("terminal", 14.)
                                                                    .text_color(content(0.5)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .min_w_0()
                                                                    .truncate()
                                                                    .child(ghost),
                                                            ),
                                                    ),
                                            )
                                            .into_any_element(),
                                    )
                                }
                            })
                            .collect::<Vec<_>>()
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(2.))
                    .pr(px(6.))
                    .child(
                        icon_button("new-terminal", NEW_TERMINAL_LABEL, "plus").on_click(
                            cx.listener(|this, _, window, cx| this.new_terminal(window, cx)),
                        ),
                    )
                    .child(
                        icon_button("move-terminal", "Move terminal", side)
                            .child(move_anchor.probe())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // ExplorerMenu hangs from the button's bottom-left corner.
                                let at = this
                                    .dock
                                    .move_terminal_anchor
                                    .get()
                                    .map(|b| point(b.left(), b.bottom() + px(4.)))
                                    .unwrap_or_default();
                                this.open_menu(
                                    MenuTarget::TerminalDock,
                                    Anchor::Point(at),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        icon_button("hide-terminal", HIDE_TERMINAL_LABEL, hide).on_click(
                            cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx)),
                        ),
                    ),
            )
            .into_any_element()
    }
}

const NEW_TERMINAL_LABEL: &str = "New terminal (⌘`)";
const HIDE_TERMINAL_LABEL: &str = "Hide terminal (⌘J)";

#[cfg(test)]
mod tests;

impl Shell {
    /// newTerminalCwd: the focused session's worktree, else its folder, else
    /// the project.
    pub(super) fn terminal_cwd(&self, cx: &App) -> String {
        self.focused_checkout(cx).to_owned()
    }

    pub(super) fn focused_checkout<'a>(&'a self, cx: &'a App) -> &'a str {
        self.workspace
            .active
            .and_then(|id| self.workspace.session_for_pane(id))
            .and_then(|id| self.sessions.get(&id))
            .map(|view| {
                let view = view.read(cx);
                view.settings
                    .worktree_cwd
                    .as_deref()
                    .unwrap_or(&view.summary.cwd)
            })
            .unwrap_or_else(|| self.working_copy())
    }
    pub(super) fn spawn_terminal(
        &mut self,
        cwd: String,
        cx: &mut Context<Self>,
    ) -> Entity<Terminal> {
        let cwd = crate::terminal::usable_cwd(cwd);
        let terminal = cx.new(|cx| Terminal::new(cwd, cx));
        self.observations
            .push(cx.subscribe(&terminal, |_, _, _: &TerminalEvent, cx| cx.notify()));
        terminal
    }
    /// A restored dock reopens its tabs as fresh shells once it is shown.
    pub(super) fn ensure_terminals(&mut self, project: &str, cx: &mut Context<Self>) {
        let Some(dock) = self.workspace.docks.get(project) else {
            return;
        };
        let wanted = dock.terminals.clone();
        let have = self.dock.terminals.get(project).map_or(0, Vec::len);
        for cwd in wanted.into_iter().skip(have) {
            let terminal = self.spawn_terminal(cwd, cx);
            self.dock
                .terminals
                .entry(project.to_owned())
                .or_default()
                .push(terminal);
        }
    }
    /// The focused terminal of the current project's dock.
    pub(super) fn dock_focused(&self, window: &Window, cx: &App) -> Option<usize> {
        self.dock
            .terminals
            .get(&self.workspace.project)?
            .iter()
            .position(|t| t.focus_handle(cx).contains_focused(window, cx))
    }
    pub(super) fn focus_terminal(&self, window: &mut Window, cx: &mut Context<Self>) {
        let project = &self.workspace.project;
        if let Some(dock) = self.workspace.docks.get(project)
            && let Some(terminal) = self
                .dock
                .terminals
                .get(project)
                .and_then(|t| t.get(dock.active))
        {
            let focus = terminal.focus_handle(cx);
            window.focus(&focus, cx);
        }
    }
    pub(super) fn slide_dock(&mut self, open: bool) {
        let project = self.workspace.project.clone();
        let Some(size) = self.workspace.docks.get(&project).map(|d| d.size) else {
            return;
        };
        let to = if open { 1. } else { 0. };
        self.dock.slide = Some((
            project.clone(),
            match self.dock.slide.take() {
                Some((p, slide)) if p == project => {
                    slide.retarget(to, vorkspace_ui::clock::Instant::now())
                }
                _ => PaneSlide::new(1. - to, to, size),
            },
        ));
    }
    /// Terminal: New (cmd-`): another terminal in the project's dock, which
    /// opens if it was hidden.
    pub(super) fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = self.workspace.project.clone();
        let cwd = self.terminal_cwd(cx);
        let was_open = self
            .workspace
            .docks
            .get(&project)
            .is_some_and(|d| d.open && !d.terminals.is_empty());
        self.ensure_terminals(&project, cx);
        let viewport = self.viewport();
        let dock = self.workspace.docks.entry(project.clone()).or_default();
        dock.size = dock.side.clamp(dock.size, viewport);
        let added = dock.add(cwd.clone());
        self.dock.tab_scroll.scroll_to_item(added);
        let terminal = self.spawn_terminal(cwd, cx);
        self.dock
            .terminals
            .entry(project)
            .or_default()
            .push(terminal);
        if !was_open {
            self.slide_dock(true);
        }
        self.focus_terminal(window, cx);
        self.persist(cx);
        cx.notify();
    }
    /// Terminal: Toggle (cmd-j).
    pub(super) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = self.workspace.project.clone();
        let Some(dock) = self
            .workspace
            .docks
            .get_mut(&project)
            .filter(|d| !d.terminals.is_empty())
        else {
            self.new_terminal(window, cx);
            return;
        };
        dock.open = !dock.open;
        let open = dock.open;
        self.slide_dock(open);
        if open {
            self.ensure_terminals(&project, cx);
            self.focus_terminal(window, cx);
        } else if self.dock_focused(window, cx).is_some() {
            self.focus_active(window, cx);
        }
        self.persist(cx);
        cx.notify();
    }
    pub(super) fn select_terminal(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dock) = self.workspace.docks.get_mut(&self.workspace.project) {
            let active = ix.min(dock.terminals.len().saturating_sub(1));
            if dock.active != active {
                dock.active = active;
                self.dock.tab_scroll.scroll_to_item(active);
                self.persist(cx);
                cx.notify();
            }
        }
        self.focus_terminal(window, cx);
    }
    /// confirmCloseTerminal: a terminal running something asks first.
    pub(super) fn close_terminal(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self
            .dock
            .terminals
            .get(&self.workspace.project)
            .and_then(|t| t.get(ix))
            .map(Entity::entity_id)
        {
            self.request_dock_close(id, None, window, cx);
        }
    }
    pub(super) fn set_dock_side(&mut self, side: DockSide, cx: &mut Context<Self>) {
        let viewport = self.viewport();
        if let Some(dock) = self.workspace.docks.get_mut(&self.workspace.project) {
            dock.set_side(side);
            dock.size = side.clamp(dock.size, viewport);
        }
        self.dock.slide = None;
        self.persist(cx);
        cx.notify();
    }
    pub(super) fn viewport(&self) -> (f32, f32) {
        (self.window.viewport_width, self.window.viewport_height)
    }
    pub(super) fn drag_dock(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((start, size)) = self.dock.resize else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.release_resizes(cx);
            return;
        }
        let Some(dock) = self.workspace.docks.get(&self.workspace.project) else {
            return;
        };
        let at = if dock.side.vertical() {
            event.position.y.as_f32()
        } else {
            event.position.x.as_f32()
        };
        let delta = at - start;
        let signed = if matches!(dock.side, DockSide::Bottom | DockSide::Right) {
            -delta
        } else {
            delta
        };
        let target = ResizeTarget::Dock(self.workspace.project.clone());
        self.drag_resize(target, size + signed, cx);
    }
    /// App.tsx's dock grid: the project's terminal dock on its side of the
    /// workspace. Opening or closing slides the track while the terminal
    /// keeps its full size and rides the moving edge, so it fits once.
    pub(super) fn with_dock(
        &mut self,
        body: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = self.workspace.project.clone();
        let workspace_visible =
            self.pages.settings.is_none() && self.workspace.active_tab().is_some();
        let dock = self.workspace.docks.get(&project).cloned();
        let progress = match self
            .dock
            .slide
            .as_ref()
            .filter(|(p, _)| dock.is_some() && *p == project)
        {
            Some((_, slide)) => {
                let (t, done) = slide.at(vorkspace_ui::clock::Instant::now());
                if done {
                    self.dock.slide = None;
                    None
                } else {
                    window.request_animation_frame();
                    Some(t)
                }
            }
            None => None,
        };
        let active = dock
            .as_ref()
            .filter(|dock| workspace_visible && (dock.open || progress.is_some()))
            .map(|dock| dock.active);
        if active.is_some() {
            self.ensure_terminals(&project, cx);
        }
        for (path, live) in &self.dock.terminals {
            // The active project's hidden grids still feed the running-terminal chip.
            let monitor = *path == project && workspace_visible;
            for (index, terminal) in live.iter().enumerate() {
                terminal.update(cx, |terminal, cx| {
                    terminal.set_activity(monitor && active == Some(index), monitor, cx)
                });
            }
        }
        let Some(dock) = dock.filter(|_| active.is_some()) else {
            return body;
        };
        let size = dock.size;
        let track = size * progress.unwrap_or(1.);
        let vertical = dock.side.vertical();
        let leading = matches!(dock.side, DockSide::Top | DockSide::Left);
        let surface = div()
            .absolute()
            .flex()
            .flex_col()
            .map(|d| {
                if vertical {
                    d.left_0().w_full().h(px(size))
                } else {
                    d.top_0().h_full().w(px(size))
                }
            })
            .map(|d| match dock.side {
                DockSide::Top => d.bottom_0(),
                DockSide::Left => d.right_0(),
                DockSide::Bottom => d.top_0(),
                DockSide::Right => d.left_0(),
            })
            .child(self.dock_header(&dock, window, cx))
            .child(
                div().flex_1().min_h_0().min_w_0().children(
                    self.dock
                        .terminals
                        .get(&project)
                        .and_then(|t| t.get(dock.active))
                        .cloned(),
                ),
            );
        let sash = div()
            .id("terminal-dock-sash")
            .debug_selector(|| "terminal-dock-sash".into())
            .group("resize-handle")
            .role(Role::Splitter)
            .aria_label("Resize terminal")
            .absolute()
            .child(resize_border(!vertical, self.dock.resize.is_some()))
            .map(|d| match dock.side {
                DockSide::Bottom => d.bottom(px(track - 3.5)).left_0().right_0().h(px(6.)),
                DockSide::Top => d.top(px(track - 3.5)).left_0().right_0().h(px(6.)),
                DockSide::Left => d.left(px(track - 3.5)).top_0().bottom_0().w(px(6.)),
                DockSide::Right => d.right(px(track - 3.5)).top_0().bottom_0().w(px(6.)),
            })
            .cursor(if vertical {
                CursorStyle::ResizeUpDown
            } else {
                CursorStyle::ResizeLeftRight
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    // A double click restores the side's default size.
                    if event.click_count == 2 {
                        if let Some(dock) = this.workspace.docks.get_mut(&this.workspace.project) {
                            dock.size = dock.side.default_size();
                        }
                        this.persist(cx);
                        cx.notify();
                        return;
                    }
                    let at = if vertical {
                        event.position.y.as_f32()
                    } else {
                        event.position.x.as_f32()
                    };
                    this.dock.resize = Some((at, size));
                }),
            );
        let frame = div()
            .id("terminal-dock")
            .debug_selector(|| "terminal-dock".into())
            .role(Role::Region)
            .aria_label("Terminal")
            .relative()
            .flex_shrink_0()
            .overflow_hidden()
            .border_color(crate::panel_kit::stroke())
            .map(|d| match dock.side {
                DockSide::Bottom => d.border_t_1(),
                DockSide::Top => d.border_b_1(),
                DockSide::Left => d.border_r_1(),
                DockSide::Right => d.border_l_1(),
            })
            .map(|d| {
                if vertical {
                    d.w_full().h(px(track))
                } else {
                    d.h_full().w(px(track))
                }
            })
            .child(surface);
        let main = div().flex_1().min_w_0().min_h_0().child(body);
        div()
            .size_full()
            .relative()
            .flex()
            .when(vertical, |d| d.flex_col())
            .map(|d| {
                if leading {
                    d.child(frame).child(main)
                } else {
                    d.child(main).child(frame)
                }
            })
            // Paint the full hit area above both surfaces, outside the dock's clip.
            .when(progress.is_none(), |d| d.child(sash))
            .into_any_element()
    }
}
