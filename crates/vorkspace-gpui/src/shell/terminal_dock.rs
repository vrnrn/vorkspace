// Project terminal dock, tab actions and resize interactions.
use super::{MenuTarget, Shell, TabShift};
use crate::{
    popover::{Anchor, MenuAction, MenuItem, MenuStyle},
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
        self.terminals
            .get(&self.workspace.project)
            .cloned()
            .unwrap_or_default()
    }
    pub(super) fn terminal_tab_menu(&self, ix: usize) -> Vec<MenuItem> {
        let count = self
            .workspace
            .docks
            .get(&self.workspace.project)
            .map_or(0, |d| d.terminals.len());
        vec![
            MenuAction::new("close", "Close").into(),
            MenuAction::new("close-others", "Close Others")
                .disabled(count < 2 || ix >= count)
                .into(),
        ]
    }
    pub(super) fn run_terminal_tab_menu(
        &mut self,
        ix: usize,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            "close" => self.close_terminal(ix, window, cx),
            "close-others" => self.close_other_terminals(ix, window, cx),
            _ => {}
        }
    }
    /// confirmCloseTerminals: the terminals still running something ask first.
    fn close_other_terminals(&mut self, keep: usize, window: &mut Window, cx: &mut Context<Self>) {
        let project = self.workspace.project.clone();
        let live = self.live_terminals();
        let Some(kept) = live.get(keep).cloned() else {
            return;
        };
        let running: Vec<(String, String)> = live
            .iter()
            .enumerate()
            .filter(|(ix, _)| *ix != keep)
            .filter_map(|(_, terminal)| {
                let t = terminal.read(cx);
                let process = t.foreground.as_deref()?.trim().to_owned();
                (!process.is_empty()).then(|| (t.title.clone(), process))
            })
            .collect();
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
            self.keep_terminal(&project, &kept, window, cx);
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
                    this.keep_terminal(&project, &kept, window, cx)
                });
            }
        })
        .detach();
    }
    fn keep_terminal(
        &mut self,
        project: &str,
        kept: &Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self
            .terminals
            .get(project)
            .and_then(|t| t.iter().position(|e| e == kept))
        else {
            return;
        };
        let had_focus = self.dock_focused(window, cx).is_some();
        if let Some(live) = self.terminals.get_mut(project) {
            live.retain(|e| e == kept);
        }
        if let Some(dock) = self.workspace.docks.get_mut(project) {
            dock.keep_only(ix);
        }
        self.dock_shifts.clear();
        if had_focus {
            self.focus_terminal(window, cx);
        }
        self.persist(cx);
        cx.notify();
    }
    /// SurfaceTabs selects a tab as it is pressed, then lets it be dragged.
    fn start_dock_drag(
        &mut self,
        ix: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dock_skip_click = false;
        self.select_terminal(ix, window, cx);
        let count = self.live_terminals().len();
        if count < 2 {
            return;
        }
        let bounds: Vec<_> = (0..count)
            .filter_map(|ix| self.dock_tab_scroll.bounds_for_item(ix))
            .collect();
        if bounds.len() != count {
            return;
        }
        self.dock_drag = Some(DockTabDrag {
            from: ix,
            to: ix,
            origin: event.position.x,
            offset: px(0.),
            active: false,
            bounds,
        });
    }
    pub(super) fn move_dock_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.dock_drag.as_mut() else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.dock_drag = None;
            cx.notify();
            return;
        }
        let delta = event.position.x - drag.origin;
        if delta.abs() < px(4.) && !drag.active {
            return;
        }
        drag.active = true;
        self.dock_skip_click = true;
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
        let Some(drag) = self.dock_drag.take() else {
            return;
        };
        if !drag.active || drag.from == drag.to {
            cx.notify();
            return;
        }
        let project = self.workspace.project.clone();
        let now = Instant::now();
        let live = self.live_terminals();
        // Each tab glides from where it is drawn into its new slot.
        let drawn: Vec<(EntityId, f32)> = live
            .iter()
            .enumerate()
            .map(|(ix, t)| {
                let id = t.entity_id();
                let shift = self.dock_shifts.get(&id).map_or(0., |s| s.at(now).0);
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
        if let Some(live) = self.terminals.get_mut(&project) {
            let moved = live.remove(drag.from);
            live.insert(drag.to, moved);
        }
        if let Some(dock) = self.workspace.docks.get_mut(&project) {
            dock.reorder(drag.from, drag.to);
        }
        self.dock_shifts.clear();
        for (ix, terminal) in self.live_terminals().iter().enumerate() {
            let id = terminal.entity_id();
            let (Some((_, left)), Some(slot)) =
                (drawn.iter().find(|(t, _)| *t == id), drag.bounds.get(ix))
            else {
                continue;
            };
            let from = left - slot.left().as_f32();
            if from.abs() > 0.5 {
                self.dock_shifts.insert(
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
        let Some(d) = self.dock_drag.as_ref().filter(|d| d.active) else {
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
        let dragged = self.dock_drag.as_ref().filter(|d| d.active).map(|d| d.from);
        let mut moving = false;
        let shifts: Vec<f32> = live
            .iter()
            .enumerate()
            .map(|(ix, terminal)| {
                let target = self.dock_tab_shift(ix).as_f32();
                let (value, active) = eased_shift(
                    &mut self.dock_shifts,
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
            self.dock_tab_motion
                .sync(keyed, animate, vorkspace_ui::clock::Instant::now());
        if moving || opening {
            window.request_animation_frame();
        }
        let move_anchor = self.move_terminal_anchor.clone();
        div()
            .h(px(36.))
            .flex_shrink_0()
            .min_w_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(crate::right_panel::stroke())
            .child(
                div()
                    .id("terminal-tabs")
                    .role(Role::TabList)
                    .aria_label("Terminals")
                    .track_scroll(&self.dock_tab_scroll)
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
                                let t = terminal.read(cx);
                                let label: SharedString = t.title.clone().into();
                                let description =
                                    SharedString::from(format!("{} — {}", t.title, t.cwd));
                                let active = ix == dock.active;
                                let held = dragged == Some(ix);
                                let hovered = self.dock_hover == Some(ix) && dragged.is_none();
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
                                        } else if this.dock_hover == Some(ix) {
                                            None
                                        } else {
                                            this.dock_hover
                                        };
                                        if next != this.dock_hover {
                                            this.dock_hover = next;
                                            cx.notify();
                                        }
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                this.start_dock_drag(ix, event, window, cx)
                                            },
                                        ),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Middle,
                                        cx.listener(move |this, _, window, cx| {
                                            this.close_terminal(ix, window, cx)
                                        }),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                cx.stop_propagation();
                                                this.select_terminal(ix, window, cx);
                                                this.open_menu(
                                                    MenuTarget::TerminalTab(ix),
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
                                            .rounded_md()
                                            .text_size(px(13.))
                                            .map(|d| {
                                                if active {
                                                    d.bg(crate::right_panel::selection())
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
                                                if std::mem::take(&mut this.dock_skip_click) {
                                                    return;
                                                }
                                                this.focus_terminal(window, cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id(("terminal-tab-close", ix))
                                            .role(Role::Button)
                                            .aria_label(SharedString::from(format!(
                                                "Close {label}"
                                            )))
                                            .group("terminal-tab-close")
                                            .absolute()
                                            .right_1()
                                            .top(px(8.))
                                            .size(px(20.))
                                            .rounded(px(4.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .opacity(if (active || hovered) && !held {
                                                1.
                                            } else {
                                                0.
                                            })
                                            .hover(|s| s.bg(content(0.1)))
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
                                                this.close_terminal(ix, window, cx);
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
                                                            .rounded_md()
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
                        icon_button("move-terminal", "Move Terminal", side)
                            .child(move_anchor.probe())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // ExplorerMenu hangs from the button's bottom-left corner.
                                let at = this
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
    pub(super) fn terminal_menu_items(
        &self,
        target: &MenuTarget,
        _cx: &App,
    ) -> Option<(SharedString, Vec<MenuItem>, f32, MenuStyle)> {
        match target {
            MenuTarget::TerminalTab(ix) => Some((
                "File tab actions".into(),
                self.terminal_tab_menu(*ix),
                160.,
                MenuStyle::Explorer,
            )),
            _ => None,
        }
    }
}

const NEW_TERMINAL_LABEL: &str = "New Terminal (⌘`)";
const HIDE_TERMINAL_LABEL: &str = "Hide Terminal (⌘J)";
