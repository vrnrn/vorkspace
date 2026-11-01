//! Rendering the split pane tree and its session surfaces.
use super::*;

impl Shell {
    /// The session pane's split header (SessionPane.tsx): grip, focus dot,
    /// title and a close button. The header moves the chat independently of
    /// its review; closing a lone chat leaves the review in the tab.
    /// `shown` is the review split's progress: the header grows and fades
    /// with the slide instead of shifting the chat at once.
    pub(super) fn split_header(
        &self,
        id: i64,
        shown: f32,
        in_split: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = self
            .sessions
            .get(&id)
            .map(|s| s.read(cx).display_title())
            .unwrap_or_default();
        let focused = self.workspace.active == Some(id) && !self.review_focused;
        div()
            .id(("split-chat-header", id as usize))
            .debug_selector(move || format!("split-header-{id}"))
            .h(px(36. * shown))
            .opacity(shown)
            .overflow_hidden()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(6.))
            .px_2()
            .border_b_1()
            .border_color(crate::panel_kit::stroke())
            .cursor(CursorStyle::OpenHand)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.review_focused = false;
                    this.focus_pane(id, window, cx);
                    this.tab_drag = None;
                    this.pane_drag = Some(PaneDrag {
                        source: DragSource::Pane(id),
                        start: event.position,
                        position: event.position,
                        active: false,
                        over: None,
                        insert: None,
                    });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id(("drag-session-pane", id as usize))
                    .debug_selector(move || format!("drag-session-pane-{id}"))
                    .role(Role::Button)
                    .aria_label("Drag to reorder pane")
                    .w(px(20.))
                    .h(px(30.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .text_color(content(0.35))
                    .hover(|s| s.bg(content(0.05)).text_color(content(0.7)))
                    .child(icon("grip-vertical", 14.)),
            )
            .child(
                div()
                    .size(px(8.))
                    .flex_shrink_0()
                    .rounded_full()
                    .when(focused, |d| d.bg(accent_color())),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(content(1.))
                    .child(title),
            )
            .child(
                div()
                    .id(("close-chat-pane", id as usize))
                    .role(Role::Button)
                    .aria_label("Close pane")
                    .group("close-chat-pane")
                    .size(px(20.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor(CursorStyle::Arrow)
                    .hover(|s| s.bg(content(0.1)))
                    .child(
                        icon("x", 12.)
                            .text_color(content(0.5))
                            .group_hover("close-chat-pane", |s| s.text_color(content(1.))),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if in_split {
                            this.close_pane(id, window, cx);
                        } else if let Some(review) = this.reviews.get(&id).cloned() {
                            this.review_focused = true;
                            review.update(cx, |r, cx| {
                                r.state.chat_hidden = true;
                                r.focus_active(window, cx);
                                cx.emit(crate::git_tools::review::ReviewEvent::Changed);
                            });
                        }
                    })),
            )
            .into_any_element()
    }
    pub(super) fn split_width(&self) -> f32 {
        self.workspace
            .active
            .and_then(|id| self.split_bounds.get(&id))
            .and_then(AnchorSlot::get)
            .map_or(800., |b| b.size.width.as_f32())
    }
    /// The card's workspace: the active tab's pane tree, or its one session.
    pub(super) fn workspace_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(id) = &self.active_terminal_tab
            && let Some(terminal) = self.terminal_tabs.get(id)
        {
            return div()
                .id("terminal-tab-body")
                .debug_selector(|| "terminal-tab-body".into())
                .size_full()
                .child(terminal.clone())
                .into_any_element();
        }
        if let Some(view) = self.app_view_body() {
            return view;
        }
        let Some(id) = self.workspace.active else {
            return div().size_full().into_any_element();
        };
        let tab = self.workspace.tab_of(id).unwrap_or(id);
        let layout = self.workspace.layout(tab);
        if layout.is_split() || self.pane_motion.as_ref().is_some_and(|m| m.tab == tab) {
            return self.scrollable_pane_tree(tab, layout, window, cx);
        }
        self.pane_scroll_focus = None;
        let hint = self.drop_hint(id);
        div()
            .size_full()
            .relative()
            .child(self.pane_viewport.layout_probe())
            .child(self.pane_bounds.layout_probe())
            .child(self.session_surface(id, None, window, cx))
            .children(hint)
            .into_any_element()
    }
    /// Keep old or resized layouts intact on a scrollable canvas when they
    /// cannot fit. The controls make both axes discoverable without a trackpad.
    pub(super) fn scrollable_pane_tree(
        &mut self,
        tab: i64,
        layout: PaneNode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (view_width, view_height) = self.pane_viewport.get().map_or((800., 600.), |b| {
            (b.size.width.as_f32(), b.size.height.as_f32())
        });
        let view_width = view_width.max(1.);
        let view_height = view_height.max(1.);
        let width = view_width.max(layout.width_for_minimum(MIN_CHAT_WIDTH));
        let stack = layout
            .max_session_stack(|id| id >= 0 && !self.workspace.review_owners.contains_key(&id));
        let height = if stack > MAX_SESSION_STACK {
            view_height.max(
                layout.height_for_minimum(view_height / MAX_SESSION_STACK as f32, |id| {
                    id >= 0 && !self.workspace.review_owners.contains_key(&id)
                }),
            )
        } else {
            view_height
        };
        let overflow_x = width > view_width + 0.5;
        let overflow_y = height > view_height + 0.5;
        let active = self.workspace.active.unwrap_or(tab);
        let signature = (tab, active, view_width.to_bits(), view_height.to_bits());
        if self.pane_scroll_focus != Some(signature) {
            if let Some(pane) = layout.leaves().iter().find(|p| p.id == active) {
                let mut x = self.pane_scroll.offset().x.as_f32();
                let mut y = self.pane_scroll.offset().y.as_f32();
                let left = pane.rect.x * width;
                let right = left + pane.rect.w * width;
                let top = pane.rect.y * height;
                let bottom = top + pane.rect.h * height;
                if left + x < 0. {
                    x = -left;
                }
                if right + x > view_width {
                    x = view_width - right;
                }
                if top + y < 0. {
                    y = -top;
                }
                if bottom + y > view_height {
                    y = view_height - bottom;
                }
                self.pane_scroll.set_offset(gpui::point(
                    px(x.clamp((view_width - width).min(0.), 0.)),
                    px(y.clamp((view_height - height).min(0.), 0.)),
                ));
            }
            self.pane_scroll_focus = Some(signature);
        }
        let tree = self.pane_tree(tab, layout.leaves(), window, cx);
        let mut buttons = Vec::new();
        if overflow_x {
            buttons.push(self.pane_scroll_button(
                "pane-scroll-left",
                "Show panes to the left",
                "chevron-left",
                (view_width * 0.7, 0.),
                (view_width - width, view_height - height),
                cx,
            ));
            buttons.push(self.pane_scroll_button(
                "pane-scroll-right",
                "Show panes to the right",
                "chevron-right",
                (-view_width * 0.7, 0.),
                (view_width - width, view_height - height),
                cx,
            ));
        }
        if overflow_y {
            buttons.push(self.pane_scroll_button(
                "pane-scroll-up",
                "Show panes above",
                "chevron-up",
                (0., view_height * 0.7),
                (view_width - width, view_height - height),
                cx,
            ));
            buttons.push(self.pane_scroll_button(
                "pane-scroll-down",
                "Show panes below",
                "chevron-down",
                (0., -view_height * 0.7),
                (view_width - width, view_height - height),
                cx,
            ));
        }
        let controls = div()
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .h(px(30.))
            .border_t_1()
            .border_color(crate::panel_kit::stroke())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.65))
                    .child("More panes"),
            )
            .children(buttons);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("pane-viewport")
                    .debug_selector(|| "pane-viewport".into())
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.pane_viewport.layout_probe())
                    .child(
                        div()
                            .id("pane-scroll-area")
                            .size_full()
                            .overflow_scroll()
                            .track_scroll(&self.pane_scroll)
                            .child(div().w(px(width)).h(px(height)).flex_shrink_0().child(tree)),
                    ),
            )
            .when(overflow_x || overflow_y, |d| d.child(controls))
            .into_any_element()
    }
    pub(super) fn scroll_panes(
        &mut self,
        delta: (f32, f32),
        limit: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        let offset = self.pane_scroll.offset();
        self.pane_scroll.set_offset(gpui::point(
            px((offset.x.as_f32() + delta.0).clamp(limit.0.min(0.), 0.)),
            px((offset.y.as_f32() + delta.1).clamp(limit.1.min(0.), 0.)),
        ));
        cx.notify();
    }

    pub(super) fn pane_scroll_button(
        &self,
        id: &'static str,
        label: &'static str,
        icon: &'static str,
        delta: (f32, f32),
        limit: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        icon_button(id, label, icon)
            .debug_selector(move || id.into())
            .focusable()
            .tab_stop(true)
            .focus_visible(|s| s.bg(content(0.15)))
            .on_click(cx.listener(move |this, _, _, cx| this.scroll_panes(delta, limit, cx)))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space" | " ") {
                    this.scroll_panes(delta, limit, cx);
                    cx.stop_propagation();
                }
            }))
            .into_any_element()
    }
    /// PaneTree.tsx: absolutely placed panes, sashes on shared edges and a
    /// drop hint on the pane under a drag.
    pub(super) fn pane_tree(
        &mut self,
        tab: i64,
        leaves: Vec<LayoutLeaf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let size = self.pane_bounds.get().map_or((800., 600.), |b| {
            (b.size.width.as_f32(), b.size.height.as_f32())
        });
        let motion = match self.pane_motion.as_ref().filter(|m| m.tab == tab) {
            Some(motion) => {
                let (t, done) = motion.slide.at(vorkspace_ui::clock::Instant::now());
                if done {
                    self.pane_motion = None;
                    None
                } else {
                    window.request_animation_frame();
                    Some(t)
                }
            }
            None => {
                self.pane_motion = None;
                None
            }
        };
        let dragging = self
            .pane_drag
            .as_ref()
            .filter(|d| d.active && !matches!(d.source, DragSource::Review(_)))
            .map(|d| d.source.session());
        let (header, leaving) = match (motion, self.pane_motion.as_ref()) {
            (Some(_), Some(m)) => (m.header, m.leaving.clone()),
            _ => (None, Vec::new()),
        };
        let t = motion.unwrap_or(1.);
        // A sliding pane holds its full size against the edge it rides; its
        // neighbours rewrap as they give way.
        let pinned = |leaf: &LayoutLeaf, edge: Edge, surface: AnyElement| {
            div()
                .absolute()
                .w(px(leaf.rect.w * size.0))
                .h(px(leaf.rect.h * size.1))
                .map(|d| match edge {
                    Edge::Left => d.top_0().right_0(),
                    Edge::Top => d.left_0().bottom_0(),
                    Edge::Right | Edge::Bottom => d.top_0().left_0(),
                })
                .flex()
                .flex_col()
                .child(surface)
        };
        let frame = |id: i64, rect: Rect| {
            div()
                .id(("pane", id as usize))
                .debug_selector(move || format!("pane-{id}"))
                .absolute()
                .left(gpui::relative(rect.x))
                .top(gpui::relative(rect.y))
                .w(gpui::relative(rect.w))
                .h(gpui::relative(rect.h))
                .overflow_hidden()
                .flex()
                .flex_col()
                .border_color(crate::panel_kit::stroke())
                .when(rect.x > 0.001, |d| d.border_l_1())
                .when(rect.y > 0.001, |d| d.border_t_1())
        };
        let mut panes = Vec::with_capacity(leaves.len() + leaving.len());
        for leaf in &leaves {
            let id = leaf.id;
            let (rect, entering) = match (motion, self.pane_motion.as_ref()) {
                (Some(t), Some(m)) => (
                    m.from
                        .get(&id)
                        .copied()
                        .unwrap_or(leaf.rect)
                        .lerp(leaf.rect, t),
                    m.entering.get(&id).copied(),
                ),
                _ => (leaf.rect, None),
            };
            // A pane with its review keeps its header either way.
            let shown = if entering.is_some() || self.review_for(id, cx).is_some() {
                1.
            } else {
                match header {
                    Some(true) => t,
                    Some(false) => 1. - t,
                    None => 1.,
                }
            };
            // Once the last neighbour has left, render the lone-pane header
            // state on this frame too, rather than flashing a full header.
            let split = (leaves.len() > 1 || motion.is_some()).then_some(shown);
            let surface = self.session_surface(id, split, window, cx);
            let content = match entering {
                Some(edge) => pinned(leaf, edge, surface),
                None => div().size_full().flex().flex_col().child(surface),
            };
            panes.push(
                frame(id, rect)
                    .when(
                        dragging == Some(id)
                            && self
                                .workspace
                                .reviews
                                .get(&id)
                                .is_none_or(|r| r.chat_hidden),
                        |d| d.opacity(0.4),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.focus_pane(id, window, cx)),
                    )
                    .child(content)
                    .children(self.drop_hint(id))
                    .into_any_element(),
            );
        }
        // Leaving panes paint over the neighbours growing into their room.
        for (leaf, edge, to) in &leaving {
            let surface = self.session_surface(leaf.id, Some(1.), window, cx);
            panes.push(
                frame(leaf.id, leaf.rect.lerp(*to, t))
                    .opacity(1. - 0.5 * t)
                    .child(pinned(leaf, *edge, surface))
                    .into_any_element(),
            );
        }
        let sashes = if motion.is_some() {
            Vec::new()
        } else {
            self.workspace.layout(tab).sashes()
        };
        div()
            .id("pane-tree")
            .size_full()
            .relative()
            .child(self.pane_bounds.layout_probe())
            .children(panes)
            .children(sashes.into_iter().map(|sash| {
                let row = sash.dir == SplitDir::Right;
                let g = sash.group;
                let key = sash.path.iter().fold(sash.index, |k, ix| k * 31 + ix + 1);
                div()
                    .id(("pane-sash", key))
                    .group("resize-handle")
                    .role(Role::Splitter)
                    .aria_label("Resize panes")
                    .absolute()
                    .map(|d| {
                        if row {
                            d.left(gpui::relative(g.x + sash.boundary * g.w))
                                .top(gpui::relative(g.y))
                                .h(gpui::relative(g.h))
                                .ml(px(-5.5))
                                .w(px(12.))
                                .cursor(CursorStyle::ResizeLeftRight)
                        } else {
                            d.top(gpui::relative(g.y + sash.boundary * g.h))
                                .left(gpui::relative(g.x))
                                .w(gpui::relative(g.w))
                                .mt(px(-5.5))
                                .h(px(12.))
                                .cursor(CursorStyle::ResizeUpDown)
                        }
                    })
                    .child(resize_border(
                        row,
                        self.pane_sash
                            .as_ref()
                            .is_some_and(|drag| drag.path == sash.path && drag.index == sash.index),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.pane_sash = Some(SashDrag {
                                tab,
                                path: sash.path.clone(),
                                index: sash.index,
                                dir: sash.dir,
                                group: g,
                            });
                        }),
                    )
            }))
            .into_any_element()
    }
    /// PaneDropHint: half the pane washed toward the edge it will take.
    pub(super) fn drop_hint(&self, id: i64) -> Option<AnyElement> {
        let drag = self.pane_drag.as_ref().filter(|d| d.active)?;
        let (target, edge) = drag.over?;
        if target != id {
            return None;
        }
        if !self.placement_allowed(drag.source, target, edge) {
            return Some(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(pane_fill())
                            .border_1()
                            .border_color(crate::panel_kit::stroke())
                            .text_size(px(12.))
                            .text_color(content(0.8))
                            .child("Not enough room to tile here"),
                    )
                    .into_any_element(),
            );
        }
        let wash = div().absolute().bg(accent_color()).opacity(0.15);
        let line = div().absolute().bg(accent_color());
        let (wash, line) = match edge {
            Edge::Left => (
                wash.top_0().bottom_0().left_0().w_1_2(),
                line.top_0().bottom_0().left_0().w(px(2.)),
            ),
            Edge::Right => (
                wash.top_0().bottom_0().right_0().w_1_2(),
                line.top_0().bottom_0().right_0().w(px(2.)),
            ),
            Edge::Top => (
                wash.left_0().right_0().top_0().h_1_2(),
                line.left_0().right_0().top_0().h(px(2.)),
            ),
            Edge::Bottom => (
                wash.left_0().right_0().bottom_0().h_1_2(),
                line.left_0().right_0().bottom_0().h(px(2.)),
            ),
        };
        let review_ratio = (drag.source == DragSource::Pane(id))
            .then(|| self.workspace.reviews.get(&id))
            .flatten()
            .filter(|state| !state.chat_hidden)
            .map(|state| state.ratio);
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .when_some(review_ratio, |d, ratio| {
                    d.left(gpui::relative(1. - ratio)).w(gpui::relative(ratio))
                })
                .child(wash)
                .child(line)
                .into_any_element(),
        )
    }
    /// One session's surface: its chat, with its review pane beside it.
    /// `split` is the split header's progress while this is a pane of a
    /// split tab; None for a tab's lone session.
    pub(super) fn session_surface(
        &mut self,
        id: i64,
        split: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let in_split = split.is_some();
        let chat = self.sessions.get(&id).cloned();
        let now = vorkspace_ui::clock::Instant::now();
        let sliding = self
            .review_slide
            .as_ref()
            .filter(|(sid, _, _)| *sid == id)
            .map(|(_, slide, review)| {
                let (width, done) = slide.at(now);
                (width, done, slide.opening(), review.clone())
            });
        // A completed close must use the same surface as the next frame.
        // Keeping the removed review here would retain its collapsing header
        // and change the layout again on the following redraw.
        let sliding = match sliding {
            Some((_, true, _, _)) => {
                self.review_slide = None;
                None
            }
            sliding => sliding,
        };
        let review = sliding
            .as_ref()
            .map(|(_, _, _, review)| review.clone())
            .or_else(|| self.review_for(id, cx));
        if let Some(chat) = &chat {
            let beside = in_split || review.is_some();
            chat.update(cx, |session, cx| session.set_beside_pane(beside, cx));
        }
        let Some(review) = review else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .when_some(split.filter(|p| *p > 0.), |d, p| {
                    d.child(self.split_header(id, p, true, cx))
                })
                .child(self.chat_with_background(id, chat, cx))
                .into_any_element();
        };
        let state = review.read(cx).state.clone();
        let (ratio, deferred) = match sliding {
            Some((width, _, opening, _)) => {
                window.request_animation_frame();
                (width, opening)
            }
            None => (state.ratio, false),
        };
        // Mount the body once the pane has arrived; cold file reads and diff
        // construction cannot compete with its opening frames.
        review.update(cx, |r, cx| {
            r.set_visible(true, cx);
            r.set_deferred(deferred, cx);
        });
        let chat_hidden = state.chat_hidden;
        let header = if in_split {
            1.
        } else if state.ratio > 0. {
            (ratio / state.ratio).clamp(0., 1.)
        } else {
            1.
        };
        let header_shown = split.unwrap_or(header);
        let ratio = if chat_hidden { 1. } else { ratio.clamp(0., 1.) };
        let bounds = self.split_bounds.entry(id).or_default().clone();
        let full_width = bounds
            .get()
            .map_or(600., |b| b.size.width.as_f32() * state.ratio);
        let dragging = self.review_drag && self.workspace.active == Some(id);
        let moving = self
            .pane_drag
            .as_ref()
            .is_some_and(|d| d.active && d.source == DragSource::Review(id));
        let moving_chat = self
            .pane_drag
            .as_ref()
            .is_some_and(|d| d.active && d.source == DragSource::Pane(id));
        let sliding_now = self
            .review_slide
            .as_ref()
            .is_some_and(|(sid, _, _)| *sid == id);
        div()
            .size_full()
            .relative()
            .flex()
            .child(bounds.layout_probe())
            .when(!chat_hidden, |d| {
                d.child(
                    div()
                        .h_full()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .when(moving_chat, |d| d.opacity(0.4))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if this.review_focused {
                                    this.review_focused = false;
                                    cx.notify();
                                }
                            }),
                        )
                        .child(self.split_header(id, header_shown, in_split, cx))
                        .child(self.chat_with_background(id, chat, cx)),
                )
            })
            .child(
                div()
                    .id(("review-frame", id as usize))
                    .debug_selector(|| "review-frame".into())
                    .relative()
                    .h_full()
                    .flex_shrink_0()
                    .w(gpui::relative(ratio))
                    .overflow_hidden()
                    .when(moving, |d| d.opacity(0.4))
                    .when(!chat_hidden, |d| {
                        d.border_l_1().border_color(crate::panel_kit::stroke())
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if !this.review_focused {
                                this.review_focused = true;
                                cx.notify();
                            }
                        }),
                    )
                    // Ride the moving leading edge at a fixed layout width.
                    // Pinning to the outer right edge would only uncover the
                    // contents, losing the slide React gives the whole surface.
                    .map(|d| {
                        // The same progress drives both directions, so a
                        // reversal cannot flash from a faded exit to opaque.
                        let d = d.opacity(if moving { header * 0.4 } else { header });
                        let review = review.cached(StyleRefinement::default().size_full());
                        if sliding_now && !chat_hidden {
                            d.child(
                                div()
                                    .id("review-content")
                                    .debug_selector(|| "review-content".into())
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .h_full()
                                    .w(px(full_width))
                                    .child(review),
                            )
                        } else {
                            d.child(div().id("review-content").size_full().child(review))
                        }
                    }),
            )
            .when(!chat_hidden && !sliding_now, |d| {
                d.child(
                    div()
                        .id(("review-sash", id as usize))
                        .group("resize-handle")
                        .role(Role::Splitter)
                        .aria_label("Resize panes")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(gpui::relative(1. - ratio))
                        .ml(px(-5.5))
                        .w(px(12.))
                        .cursor(CursorStyle::ResizeLeftRight)
                        .child(resize_border(true, dragging))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.focus_pane(id, window, cx);
                                this.review_drag = true;
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                )
            })
            .into_any_element()
    }
    /// The narrowest chat column the active tab would have, with or without
    /// the sidebar: each pane's share of the card, less its review.
    pub(super) fn narrowest_chat(&self, sidebar: bool, cx: &App) -> f32 {
        // The card's inset and border.
        let mut main = self.viewport_width - self.classic_rail_width();
        if sidebar {
            main -= self.workspace.right_panel_width;
        }
        if self.right_panel_shown(cx) {
            main -= self.workspace.right_panel_width;
        }
        if self.bot_active(cx) {
            return main;
        }
        if let Some(dock) = self.workspace.docks.get(&self.workspace.project)
            && dock.open
            && !dock.side.vertical()
        {
            main -= dock.size;
        }
        let Some(tab) = self
            .workspace
            .active_tab()
            .filter(|_| !self.app_view_open())
        else {
            return main;
        };
        self.workspace
            .layout(tab)
            .leaves()
            .iter()
            .map(|leaf| {
                let width = leaf.rect.w * main;
                let review = self
                    .reviews
                    .get(&leaf.id)
                    .map(|r| r.read(cx).state.clone())
                    .or_else(|| self.workspace.reviews.get(&leaf.id).cloned())
                    .filter(|r| !r.tabs.is_empty());
                match review {
                    Some(r) if r.chat_hidden => width,
                    Some(r) => width * (1. - r.ratio),
                    None => width,
                }
            })
            .fold(f32::INFINITY, f32::min)
    }
}
