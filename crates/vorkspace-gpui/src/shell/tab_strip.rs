//! The tab strip: labels, close rules, dragging and shift animation.
use super::*;

impl Shell {
    pub(super) fn close_other_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.active_title_tab(cx) else {
            return;
        };
        let tabs = self
            .visible_tabs(cx)
            .into_iter()
            .filter(|tab| *tab != active)
            .collect::<Vec<_>>();
        self.close_title_tabs(&tabs, Some(active), window, cx);
    }
    pub(super) fn activate_tab(
        &mut self,
        slot: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = self.visible_tabs(cx);
        let index = if slot < 0 {
            tabs.len().saturating_sub(1)
        } else {
            slot as usize
        };
        if let Some(tab) = tabs.get(index) {
            self.open_title_tab(tab, window, cx);
        }
    }
    /// App.tsx's toTitleTab with TitleBar's tabCopy: a split tab, or a chat
    /// with a file beside it, heads with the focused pane and names the rest
    /// on a second line.
    pub(super) fn tab_copy(&self, tab: i64, cx: &App) -> TabCopy {
        let focus = self.workspace.tab_focus(tab);
        let review = |id: &i64| {
            self.workspace
                .reviews
                .get(id)
                .filter(|r| !r.tabs.is_empty())
        };
        let mut leaves = self.workspace.layout(tab).leaf_ids();
        // Focused first, as harnesses and files are ordered.
        if let Some(ix) = leaves.iter().position(|id| *id == focus) {
            let id = leaves.remove(ix);
            leaves.insert(0, id);
        }
        // A chat closed beside its file leaves only the file pane.
        let sessions: Vec<i64> = leaves
            .iter()
            .copied()
            .filter(|id| !review(id).is_some_and(|r| r.chat_hidden))
            .filter(|id| self.session_summaries.contains_key(id))
            .collect();
        let mut files = Vec::new();
        for id in &leaves {
            if let Some(r) = review(id) {
                let mut labels = r.tabs.iter().map(|t| t.label());
                let active = r.tabs.get(r.active).map(|t| t.label());
                for label in active.into_iter().chain(&mut labels) {
                    if !files.contains(&label) {
                        files.push(label);
                    }
                }
            }
        }
        let file_panes = leaves.iter().filter(|id| review(id).is_some()).count();
        let multi_pane = sessions.len() + file_panes > 1;
        let session_focused = sessions.first() == Some(&focus);
        let file_focused =
            !session_focused || (self.tab_review_focused(tab, cx) && review(&focus).is_some());
        let title = |id: &i64| {
            let title = self.session_metadata(*id, cx).display_title();
            if title == "New session" {
                String::new()
            } else {
                title
            }
        };
        let conversation = sessions.first().map(title).unwrap_or_default();
        let file = files.first().cloned().unwrap_or_default();
        let untitled = "New session".to_owned();
        let headline = if multi_pane && file_focused && !file.is_empty() {
            file
        } else {
            [conversation, file]
                .into_iter()
                .find(|t| !t.is_empty())
                .unwrap_or_else(|| untitled.clone())
        };
        // Layout order, a chat before the file beside it, so moving focus
        // right or down rolls the headline up.
        let mut panes = Vec::new();
        for id in self.workspace.layout(tab).leaf_ids() {
            if sessions.contains(&id) {
                let session = self.session_metadata(id, cx);
                let lit = session.responding()
                    || session.needs_approval()
                    || self.unseen_finished.contains(&id);
                let title = title(&id);
                panes.push(TabPane {
                    key: (id, false),
                    title: if title.is_empty() {
                        untitled.clone()
                    } else {
                        title
                    },
                    mark: PaneMark::Session(session.summary.provider.clone(), lit.then_some(id)),
                });
            }
            if let Some(r) = review(&id)
                && let Some(t) = r.tabs.get(r.active)
            {
                panes.push(TabPane {
                    key: (id, true),
                    title: t.label(),
                    mark: PaneMark::File(
                        matches!(t, crate::git_tools::review::ReviewTab::File { .. })
                            .then(|| t.label()),
                    ),
                });
            }
        }
        let focused = (focus, file_focused && review(&focus).is_some());
        let focus = panes.iter().position(|p| p.key == focused).unwrap_or(0);
        TabCopy {
            headline,
            panes,
            focus,
        }
    }
    /// A pane's mark: its provider, spinning while it works and checked when
    /// it finished unseen, or the file's icon.
    pub(super) fn pane_mark(&self, mark: &PaneMark, active: bool) -> AnyElement {
        let ink = content(if active { 1. } else { 0.5 });
        let mark = match mark {
            PaneMark::Session(provider, lit) => match lit.and_then(|id| self.tab_marks.get(&id)) {
                Some(mark) => mark
                    .clone()
                    .cached(StyleRefinement::default().size(px(14.)))
                    .into_any_element(),
                None => provider_icon(provider, 14.)
                    .opacity(if active { 1. } else { 0.55 })
                    .into_any_element(),
            },
            PaneMark::File(Some(name)) => {
                vorkspace_ui::file_icons::file_type_icon(name, false, false, 14.)
            }
            PaneMark::File(None) => icon("git-compare", 14.).text_color(ink).into_any_element(),
        };
        div()
            .size(px(14.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(mark)
            .into_any_element()
    }
    /// The focused pane's mark and title on one line; while focus moves,
    /// the line leaving rolls out as the new one rolls in behind it.
    pub(super) fn pane_headline(
        &self,
        copy: &TabCopy,
        (leaving, dir, progress): (Option<usize>, f32, f32),
        active: bool,
        hovered: bool,
    ) -> AnyElement {
        const LINE: f32 = 20.;
        let line = |pane: &TabPane, top: f32, opacity: f32| {
            div()
                .absolute()
                .left_0()
                .top(px(top))
                .w_full()
                .h(px(LINE))
                .opacity(opacity)
                .flex()
                .items_center()
                .gap(px(6.))
                .child(self.pane_mark(&pane.mark, active))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(vorkspace_ui::tab_label::tab_label(pane.title.clone())),
                )
        };
        div()
            .flex_1()
            .min_w_0()
            .h(px(LINE))
            .relative()
            .overflow_hidden()
            .text_size(px(TITLE_TAB_TEXT_SIZE))
            .text_color(content(if active || hovered { 1. } else { 0.5 }))
            .group_hover("title-tab", |s| s.text_color(content(1.)))
            .when_some(copy.panes.get(copy.focus), |d, pane| {
                d.child(line(pane, dir * (1. - progress) * LINE, progress.max(0.2)))
            })
            .when_some(leaving.and_then(|ix| copy.panes.get(ix)), |d, pane| {
                d.child(line(pane, -dir * progress * LINE, 1. - progress))
            })
            .into_any_element()
    }
    /// Where tab `tab`'s headline is in its roll to the focused pane: the
    /// pane leaving, its direction, and the eased progress.
    pub(super) fn pane_tick(
        &mut self,
        tab: i64,
        copy: &TabCopy,
        animate: bool,
    ) -> (Option<usize>, f32, f32) {
        let Some(shown) = copy.panes.get(copy.focus).map(|p| p.key) else {
            self.pane_ticks.remove(&tab);
            return (None, 0., 1.);
        };
        let now = vorkspace_ui::clock::Instant::now();
        let position = |key: (i64, bool)| copy.panes.iter().position(|p| p.key == key);
        let tick = self.pane_ticks.entry(tab).or_insert(PaneTick {
            shown,
            from: None,
            dir: 0.,
            start: now,
        });
        if tick.shown != shown {
            let from = position(tick.shown).filter(|_| animate);
            tick.dir = if from.is_some_and(|ix| ix > copy.focus) {
                -1.
            } else {
                1.
            };
            tick.from = from.map(|_| tick.shown);
            tick.shown = shown;
            tick.start = now;
        }
        if !animate {
            tick.from = None;
        }
        let t = now.saturating_duration_since(tick.start).as_secs_f32() / PANE_TICK_DURATION;
        let from = tick.from.and_then(position).filter(|_| t < 1.);
        if from.is_none() {
            tick.from = None;
            return (None, 0., 1.);
        }
        (from, tick.dir, vorkspace_ui::reveal::ease_move(t))
    }
    /// The dragged session's title, following the pointer.
    pub(super) fn drag_ghost(&self, cx: &App) -> Option<AnyElement> {
        let drag = self.pane_drag.as_ref().filter(|d| d.active)?;
        let id = drag.source.session();
        let review = (matches!(drag.source, DragSource::Review(_))
            || self.workspace.review_owners.contains_key(&id))
        .then(|| self.workspace.reviews.get(&id))
        .flatten()
        .and_then(|r| r.tabs.get(r.active));
        let owner = self.workspace.session_for_pane(id);
        self.session_summaries.get(&owner)?;
        let session = self.session_metadata(owner, cx);
        let title = if let Some(tab) = review {
            tab.label()
        } else if self.workspace.review_owners.contains_key(&id) {
            self.tab_copy(id, cx).headline
        } else {
            session.display_title()
        };
        Some(
            div()
                .absolute()
                .left(drag.position.x + px(14.))
                .top(drag.position.y + px(10.))
                .max_w(px(240.))
                .h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .bg(pane_fill())
                .shadow_md()
                .text_size(px(12.))
                .text_color(content(1.))
                .child(if let Some(tab) = review {
                    if matches!(tab, crate::git_tools::review::ReviewTab::File { .. }) {
                        vorkspace_ui::file_icons::file_type_icon(&tab.label(), false, false, 14.)
                    } else {
                        icon("git-compare", 14.).into_any_element()
                    }
                } else {
                    provider_icon(&session.summary.provider, 14.).into_any_element()
                })
                .child(div().min_w_0().truncate().child(title))
                .into_any_element(),
        )
    }
    pub(super) fn tab_can_close(&self, id: i64, cx: &App) -> bool {
        let Some(tab) = self.workspace.tab_of(id) else {
            return false;
        };
        let layout = self.workspace.layout(tab);
        self.visible_tabs(cx).len() > 1
            || layout.is_split()
            || layout.leaf_ids().iter().any(|id| {
                self.session_summaries.contains_key(id) && {
                    let s = self.session_metadata(*id, cx);
                    s.summary.has_messages
                        || s.busy
                        || s.has_saved_draft()
                        || self
                            .sessions
                            .get(id)
                            .is_some_and(|v| !v.read(cx).input.read(cx).content.is_empty())
                } || self
                    .workspace
                    .reviews
                    .get(id)
                    .is_some_and(|r| !r.tabs.is_empty())
            })
    }
    pub(super) fn start_tab_drag(
        &mut self,
        id: TitleTab,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.skip_tab_click = false;
        self.close_menu(cx);
        let tabs = self.visible_tabs(cx);
        let Some(from) = tabs.iter().position(|tab| *tab == id) else {
            return;
        };
        if tabs.len() < 2 {
            return;
        }
        let bounds: Vec<_> = (0..tabs.len())
            .filter_map(|ix| self.tab_scroll.bounds_for_item(ix))
            .collect();
        if bounds.len() != tabs.len() {
            return;
        }
        self.tab_drag = Some(TabDrag {
            id,
            from,
            to: from,
            origin: event.position.x,
            offset: px(0.),
            active: false,
            bounds,
            armed: false,
        });
        self.tabs_frame.update(cx, |_, cx| cx.notify());
    }
    pub(super) fn move_tab_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.tab_drag.as_mut() else {
            return;
        };
        // A press whose release never reached the shell (the title bar can
        // swallow it) must not wake up on the next drag elsewhere.
        if event.pressed_button != Some(MouseButton::Left) {
            self.tab_drag = None;
            self.cancel_pane_drag(cx);
            self.tabs_frame.update(cx, |_, cx| cx.notify());
            return;
        }
        let delta = event.position.x - drag.origin;
        let pulled = event.position.y.as_f32() - TITLE_BAR_HEIGHT;
        if delta.abs() < px(4.) && !drag.active && pulled < 4. {
            return;
        }
        drag.active = true;
        self.skip_tab_click = true;
        // Once armed it stays armed until the pointer is back over the tabs.
        if pulled <= 0. {
            drag.armed = false;
        } else if pulled >= PANE_DROP_PULL {
            drag.armed = true;
        }
        if drag.armed {
            // Pulled out of the strip: the tab settles back into its slot
            // and the strip holds still while the drag targets a pane.
            drag.offset = px(0.);
            drag.to = drag.from;
        } else {
            let start = drag.bounds[drag.from].left();
            drag.offset = delta.clamp(
                drag.bounds[0].left() - start,
                drag.bounds.last().unwrap().left() - start,
            );
            drag.to = drag
                .bounds
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    (a.left() - start - drag.offset)
                        .abs()
                        .partial_cmp(&(b.left() - start - drag.offset).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(ix, _)| ix)
                .unwrap_or(drag.from);
        }
        let (id, armed) = (drag.id.clone(), drag.armed);
        // The active tab has no other pane to land on, but its title still
        // follows the pointer.
        let own = self.workspace.active_tab() == id.session();
        let was = self.pane_drag.is_some();
        self.pane_drag = id.session().filter(|_| armed).map(|id| PaneDrag {
            source: DragSource::Tab(id),
            start: event.position,
            position: event.position,
            active: true,
            over: if own {
                None
            } else {
                self.pane_hit(event.position)
            },
            insert: None,
        });
        if armed || was {
            cx.notify();
        }
        self.tabs_frame.update(cx, |_, cx| cx.notify());
    }
    pub(super) fn finish_tab_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(drag) = self.tab_drag.take() {
            let onto_pane = self.pane_drag.as_ref().is_some_and(|d| {
                Some(d.source.session()) == drag.id.session()
                    && matches!(d.source, DragSource::Tab(_))
                    && d.over.is_some()
            });
            if onto_pane {
                self.skip_tab_click = true;
                self.finish_pane_drag(window, cx);
            } else if drag.active {
                self.cancel_pane_drag(cx);
                // Each tab glides from where it is drawn into its new slot.
                let now = vorkspace_ui::clock::Instant::now();
                let visible = self.visible_tabs(cx);
                let drawn: Vec<(TitleTab, f32)> = visible
                    .iter()
                    .enumerate()
                    .map(|(ix, id)| {
                        let shift = self.tab_shifts.get(id).map_or(0., |s| s.at(now).0);
                        (
                            id.clone(),
                            drag.bounds.get(ix).map_or(0., |b| b.left().as_f32()) + shift,
                        )
                    })
                    .collect();
                self.reorder_title_tab(&drag.id, drag.to, cx);
                self.tab_shifts.clear();
                for (ix, id) in self.visible_tabs(cx).iter().enumerate() {
                    let (Some((_, left)), Some(slot)) =
                        (drawn.iter().find(|(t, _)| t == id), drag.bounds.get(ix))
                    else {
                        continue;
                    };
                    let from = left - slot.left().as_f32();
                    if from.abs() > 0.5 {
                        self.tab_shifts.insert(
                            id.clone(),
                            TabShift {
                                from,
                                to: 0.,
                                start: now,
                            },
                        );
                    }
                }
                self.skip_tab_click = true;
                self.persist(cx);
            }
            self.tabs_frame.update(cx, |_, cx| cx.notify());
        }
    }
    /// Where tab `id` is drawn now, easing toward `target`.
    pub(super) fn eased_shift(&mut self, id: TitleTab, target: f32, follow: bool) -> (f32, bool) {
        let now = vorkspace_ui::clock::Instant::now();
        // The dragged tab rides the pointer; it eases only into a slot.
        if follow {
            self.tab_shifts.remove(&id);
            if follow {
                self.tab_shifts.insert(
                    id.clone(),
                    TabShift {
                        from: target,
                        to: target,
                        start: now - std::time::Duration::from_secs(1),
                    },
                );
            }
            return (target, false);
        }
        let current = self.tab_shifts.get(&id).map(|s| s.at(now));
        if current.is_none_or(|_| self.tab_shifts[&id].to != target) {
            let from = current.map_or(0., |(value, _)| value);
            if from == target {
                self.tab_shifts.remove(&id);
                return (target, false);
            }
            self.tab_shifts.insert(
                id.clone(),
                TabShift {
                    from,
                    to: target,
                    start: now,
                },
            );
        }
        let (value, done) = self.tab_shifts[&id].at(now);
        if done && target == 0. {
            self.tab_shifts.remove(&id);
        }
        (value, !done)
    }
    pub(super) fn tab_shift(&self, ix: usize) -> Pixels {
        let Some(d) = self.tab_drag.as_ref().filter(|d| d.active) else {
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
    /// The tab strip's width: between the navigation and TitleBar.tsx's right
    /// panel toggle, `pl-1.5 pr-3` clear of the edge.
    pub(super) fn tabs_width(&self, window: &Window, cx: &App) -> f32 {
        let sidebar = self.workspace.right_panel_width * self.sidebar_fraction;
        let left = !self.workspace.preferences.sidebar_right;
        let trailing = if self.workspace.classic_layout {
            (if left { 0. } else { sidebar }) + self.right_drawn + self.strip_trail(cx)
        } else {
            self.toggle_room(cx)
        };
        // Tabs start beside the project rail; both right-hand sidebars consume
        // trailing space without moving the tab strip's leading edge.
        let lead = if self.workspace.classic_layout {
            self.classic_rail_width() + if left { sidebar } else { 0. } + self.strip_lead(window)
        } else {
            self.navigation_width(window)
        };
        (window.viewport_size().width.as_f32() - lead - trailing).max(0.)
    }
    pub(super) fn tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let viewport = self.tabs_width(window, cx);
        let tabs = self.visible_tabs(cx);
        let count = tabs.len();
        let insert = self.pane_drag.as_ref().and_then(|d| d.insert);
        let animate = self.workspace.preferences.tab_animations
            && !self.tab_drag.as_ref().is_some_and(|d| d.active);
        let (slots, opening) = self.title_tab_motion.sync(
            tabs.iter().map(|tab| (tab.clone(), ())).collect(),
            animate,
            vorkspace_ui::clock::Instant::now(),
        );
        // Every tab fits: they share the strip, however many there are. A tab
        // easing in or out holds its share of a slot, so the others give way
        // to it gradually, as flex-shrink does in TitleBar.tsx.
        let share: f32 = slots
            .iter()
            .map(|s| match s {
                vorkspace_ui::tab_motion::Slot::Live { open, .. }
                | vorkspace_ui::tab_motion::Slot::Closing { open, .. } => *open,
            })
            .sum();
        let avail = viewport - STRIP_PAD - STRIP_NEW_TAB - share * 2.;
        let width = if share <= 0. {
            TITLE_TAB_MAX_WIDTH
        } else {
            (avail / share).clamp(0., TITLE_TAB_MAX_WIDTH)
        };
        // The selected tab keeps a wider share; the others split the rest.
        let selected = self.active_title_tab(cx);
        let now = vorkspace_ui::clock::Instant::now();
        let selection_changed = self.title_tab_selection.1 != selected;
        if selection_changed {
            let previous = self.title_tab_selection.1.take();
            self.title_tab_selection = (previous.filter(|_| animate), selected.clone(), now);
        }
        let switch = vorkspace_ui::tab_motion::progress(self.title_tab_selection.2, now);
        if switch >= 1. {
            self.title_tab_selection.0 = None;
        }
        let leaving = self.title_tab_selection.0.clone();
        let switching = leaving.is_some();
        // How much of the selected tab's wider share each tab holds: the new
        // one eases in while the previous one eases out.
        let weight = |tab: &TitleTab| {
            if Some(tab) == selected.as_ref() {
                if switching { switch } else { 1. }
            } else if leaving.as_ref() == Some(tab) {
                1. - switch
            } else {
                0.
            }
        };
        let held: f32 = slots
            .iter()
            .map(|s| match s {
                vorkspace_ui::tab_motion::Slot::Live { key, open } => weight(key) * open,
                vorkspace_ui::tab_motion::Slot::Closing { .. } => 0.,
            })
            .sum();
        // The selected tab shrinks with the rest down to a floor, then holds
        // there while the others keep giving way.
        let (width, floor) = if width < SELECTED_TAB_MIN_WIDTH && held > 0. && share - held > 0. {
            let floor = SELECTED_TAB_MIN_WIDTH.min(avail.max(0.));
            (((avail - floor * held) / (share - held)).max(0.), floor)
        } else {
            (width, width)
        };
        // Fractional widths round differently from tab to tab and the error
        // adds up along the strip, so a full strip's new-tab button would
        // wander as tabs come and go. Every slot snaps to device pixels and
        // the button's margin takes what is left, keeping it still.
        let scale = window.scale_factor();
        let snap = |x: f32| (x * scale).floor() / scale;
        let crowded = width < TITLE_TAB_MAX_WIDTH;
        let base_width = |tab: &TitleTab| snap(width + (floor - width) * weight(tab));
        let slot_size = |tab: &TitleTab, open: f32| {
            (
                snap(base_width(tab) * open),
                snap(vorkspace_ui::tab_motion::GAP * (1. - open)),
            )
        };
        let used: f32 = slots
            .iter()
            .map(|s| match s {
                vorkspace_ui::tab_motion::Slot::Live { key, open }
                | vorkspace_ui::tab_motion::Slot::Closing { key, open, .. } => {
                    if *open >= 1. {
                        base_width(key) + 2.
                    } else {
                        let (w, give) = slot_size(key, *open);
                        w + 2. - give
                    }
                }
            })
            .sum();
        // The settled tabs take back what snapping left over, a device pixel
        // each from the left, so the new-tab button keeps its usual 2px gap.
        let mut extra: HashMap<TitleTab, f32> = HashMap::new();
        let mut new_tab_lead = 2.;
        if crowded {
            let leftover = (viewport - STRIP_PAD - STRIP_NEW_TAB - used).max(0.);
            let settled: Vec<&TitleTab> = slots
                .iter()
                .filter_map(|s| match s {
                    vorkspace_ui::tab_motion::Slot::Live { key, open } if *open >= 1. => Some(key),
                    _ => None,
                })
                .collect();
            let units = (leftover * scale).floor() as usize;
            if !settled.is_empty() {
                for i in 0..units {
                    *extra.entry(settled[i % settled.len()].clone()).or_default() += 1. / scale;
                }
                new_tab_lead += leftover - units as f32 / scale;
            } else {
                new_tab_lead += leftover;
            }
        }
        let tab_width = |tab: &TitleTab| base_width(tab) + extra.get(tab).copied().unwrap_or(0.);
        let dragged = self
            .tab_drag
            .as_ref()
            .filter(|d| d.active && !d.armed)
            .map(|d| d.id.clone());
        let mut moving = false;
        let shifts: Vec<Pixels> = tabs
            .clone()
            .into_iter()
            .enumerate()
            .map(|(ix, id)| {
                let target = self.tab_shift(ix).as_f32();
                let (value, active) =
                    self.eased_shift(id.clone(), target, dragged.as_ref() == Some(&id));
                moving |= active;
                px(value)
            })
            .collect();
        // Keep inactive headlines on their last pane. Only an in-tab focus
        // change rolls the title; selecting a tab settles it in the same frame.
        self.pane_ticks
            .retain(|id, _| self.workspace.tabs.contains(id));
        let mut copies = HashMap::new();
        for id in tabs.iter().filter_map(|tab| tab.session()) {
            if self
                .session_summaries
                .contains_key(&self.workspace.session_for_pane(id))
            {
                let copy = self.tab_copy(id, cx);
                let tick = self.pane_tick(
                    id,
                    &copy,
                    animate && !selection_changed && selected == Some(TitleTab::Session(id)),
                );
                moving |= tick.0.is_some();
                copies.insert(id, (copy, tick));
            }
        }
        let mut session_elements: HashMap<i64, AnyElement> =
            tabs.iter()
                .enumerate()
                .filter_map(|(ix, tab)| {
                    tab.session().and_then(|id| {
                        self.session_summaries
                            .contains_key(&self.workspace.session_for_pane(id))
                            .then_some((ix, id))
                    })
                })
                .filter_map(|(ix, id)| {
                    let (copy, (leaving, dir, progress)) = copies.remove(&id)?;
                    let active = self.workspace.active_tab() == Some(id)
                        && !self.app_view_open()
                        && !self.bot_active(cx)
                        && self.app_search.is_none();
                    // The press itself redraws the strip to arm tab dragging.
                    // Keep its ink stable until that press is released, even if
                    // hover hitboxes are being replaced during that redraw.
                    let hovered = self.tab_hover == Some(id)
                        || self
                            .tab_drag
                            .as_ref()
                            .is_some_and(|drag| drag.id == TitleTab::Session(id) && !drag.active);
                    // A collapsed tab drops its close button, so a click anywhere
                    // on it selects it; selected, it is wide enough to show it again.
                    let close = self.tab_can_close(id, cx)
                        && (active || tab_width(&TitleTab::Session(id)) >= TAB_CLOSE_MIN_WIDTH);
                    let dragging = self
                        .tab_drag
                        .as_ref()
                        .is_some_and(|d| d.id == TitleTab::Session(id) && d.active);
                    let tab = vorkspace_ui::theme::keyboard_reachable(
                        div().id(("tab", id as usize)),
                        true,
                    )
                    .debug_selector(move || format!("title-tab-{id}"))
                    .group("title-tab")
                    .role(Role::Tab)
                    .aria_label(copy.headline.clone())
                    .aria_selected(active)
                    .cursor(CursorStyle::Arrow)
                    .w(px(tab_width(&TitleTab::Session(id))))
                    .flex_shrink_0()
                    .h(px(TITLE_BAR_HEIGHT))
                    .relative()
                    .left(shifts[ix])
                    .flex()
                    .items_center()
                    .child(
                        title_tab_plate(active, hovered)
                            .id(("tab-plate", id as usize))
                            .relative()
                            .child(self.tab_liquid.row(format!("tab:{id}"), active))
                            .w_full()
                            .min_w_0()
                            // A narrow tab keeps its mark; the close button
                            // overlays it on hover instead of taking room.
                            .when(
                                close && tab_width(&TitleTab::Session(id)) >= TAB_CLOSE_MIN_WIDTH,
                                |d| d.pr_7(),
                            )
                            // Too narrow for the plate's padding: the mark sits centred.
                            .when(tab_width(&TitleTab::Session(id)) < 30., |d| {
                                d.px(px(((tab_width(&TitleTab::Session(id)) - 14.) / 2.).max(0.)))
                            })
                            .gap(px(6.))
                            .child(self.pane_headline(
                                &copy,
                                (leaving, dir, progress),
                                active,
                                hovered,
                            )),
                    )
                    .when(close, |d| {
                        d.child(
                            div()
                                .id(("close-tab", id as usize))
                                .debug_selector(move || format!("close-title-tab-{id}"))
                                .group("tab-close")
                                .role(Role::Button)
                                .aria_label(format!("Close tab {}", copy.headline))
                                .absolute()
                                .right_1()
                                .top(px(10. + TAB_LABEL_DROP))
                                .size(px(20.))
                                .rounded_sm()
                                .flex()
                                .items_center()
                                .justify_center()
                                .hover(|s| s.bg(content(0.1)))
                                .when(copy.panes.len() > 1, |d| {
                                    d.child(
                                        div()
                                            .debug_selector(move || {
                                                format!("title-tab-pane-count-{id}")
                                            })
                                            .text_size(px(11.))
                                            .line_height(px(20.))
                                            .text_color(content(0.4))
                                            .group_hover("title-tab", |s| s.opacity(0.))
                                            .child(copy.panes.len().to_string()),
                                    )
                                })
                                .child(
                                    div()
                                        .absolute()
                                        .top_1()
                                        .left_1()
                                        .size(px(12.))
                                        .opacity(0.)
                                        .group_hover("title-tab", |s| s.opacity(1.))
                                        .child(icon("close", 12.).group_hover("tab-close", |s| {
                                            s.text_color(content(1.))
                                        })),
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close(id, window, cx);
                                })),
                        )
                    })
                    .map(|d| {
                        let bar = || {
                            div()
                                .absolute()
                                .top(px(9.))
                                .h(px(22.))
                                .w(px(2.))
                                .rounded_full()
                                .bg(accent_color())
                        };
                        d.when(insert == Some(ix), |d| d.child(bar().left(px(-2.))))
                            .when(insert == Some(count) && ix + 1 == count, |d| {
                                d.child(bar().right(px(-2.)))
                            })
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, _, cx| {
                            this.start_tab_drag(TitleTab::Session(id), event, cx)
                        }),
                    )
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        if *hovered {
                            this.tab_hover = Some(id);
                        } else if this.tab_hover == Some(id) {
                            this.tab_hover = None;
                        }
                        this.tabs_frame.update(cx, |_, cx| cx.notify());
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.open_menu(
                                MenuTarget::Tab(id),
                                Anchor::Point(event.position),
                                window,
                                cx,
                            );
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close(id, window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if std::mem::take(&mut this.skip_tab_click) {
                            return;
                        }
                        this.open_title_tab(&TitleTab::Session(id), window, cx);
                    }));
                    let element = if dragging {
                        gpui::deferred(tab).with_priority(1).into_any()
                    } else {
                        tab.into_any()
                    };
                    Some((id, element))
                })
                .collect();
        let elements: Vec<AnyElement> = tabs
            .iter()
            .enumerate()
            .filter_map(|(ix, tab)| match tab {
                TitleTab::Session(id) => session_elements.remove(id),
                TitleTab::Terminal { id, cwd } => {
                    let element = self
                        .terminal_title_tab(*id, cwd, tab_width(tab), cx)
                        .left(shifts[ix]);
                    Some(if dragged.as_ref() == Some(tab) {
                        gpui::deferred(element).with_priority(1).into_any_element()
                    } else {
                        element.into_any_element()
                    })
                }
                TitleTab::Search | TitleTab::App(_) | TitleTab::Bot { .. } => None,
            })
            .collect();
        if moving || opening || switching {
            window.request_animation_frame();
        }
        let mut live: HashMap<TitleTab, AnyElement> = tabs.iter().cloned().zip(elements).collect();
        // TabWidthMotion: the slot's width eases while the tab keeps its own.
        let slot = |tab: &TitleTab, open: f32| {
            let (w, give) = slot_size(tab, open);
            div()
                .flex_shrink_0()
                .h(px(TITLE_BAR_HEIGHT))
                .w(px(w))
                .mr(px(-give))
                .overflow_hidden()
        };
        let elements: Vec<AnyElement> = slots
            .into_iter()
            .filter_map(|s| match s {
                vorkspace_ui::tab_motion::Slot::Live { key, open } => {
                    let element = live.remove(&key)?;
                    Some(if open >= 1. {
                        element
                    } else {
                        slot(&key, open)
                            // In a full strip the tab opens out of the new-tab
                            // button: its right edge shows first, so it grows
                            // leftward instead of sliding under the button.
                            .when(crowded, |d| d.flex().justify_end())
                            .child(element)
                            .into_any_element()
                    })
                }
                vorkspace_ui::tab_motion::Slot::Closing { key, open, .. } => {
                    let w = tab_width(&key);
                    // A closed conversation stays loaded, so its copy still reads.
                    let headline = key
                        .session()
                        .filter(|id| {
                            self.session_summaries
                                .contains_key(&self.workspace.session_for_pane(*id))
                        })
                        .map(|id| {
                            let copy = self.tab_copy(id, cx);
                            self.pane_headline(&copy, (None, 0., 1.), false, false)
                        });
                    Some(
                        slot(&key, open)
                            .child(
                                div()
                                    .w(px(w))
                                    .h(px(TITLE_BAR_HEIGHT))
                                    .flex()
                                    .items_center()
                                    .child(
                                        title_tab_plate(false, false)
                                            .w_full()
                                            .min_w_0()
                                            .gap(px(6.))
                                            .children(headline),
                                    ),
                            )
                            .into_any_element(),
                    )
                }
            })
            .collect();
        div()
            .id("title-tab-strip")
            .debug_selector(|| "title-tab-strip".into())
            .w(px(viewport))
            .min_w_0()
            .h(px(TITLE_BAR_HEIGHT))
            .relative()
            .flex()
            .items_center()
            .overflow_hidden()
            // The line beneath the tabs, and their pills rising out of it.
            .child(self.tab_liquid.surface(None, false))
            .child(
                div()
                    .id("session-tabs")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .track_scroll(&self.tab_scroll)
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .pl(px(STRIP_LEAD))
                    .pr(px(STRIP_TRAIL - 2.))
                    .children(elements)
                    .child(
                        div()
                            .id("new-tab")
                            .debug_selector(|| "new-tab".into())
                            .group("new-tab")
                            .role(Role::Button)
                            .aria_label("New session tab; Command-click for a terminal")
                            .cursor(CursorStyle::Arrow)
                            .size(px(TAB_PLATE_HEIGHT))
                            .flex_shrink_0()
                            .ml(px(new_tab_lead))
                            .rounded_md()
                            .relative()
                            .child(self.tab_liquid.row("new-tab", false))
                            .opacity(0.65)
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.opacity(1.))
                            .child(
                                icon("plus", 14.)
                                    .group_hover("new-tab", |s| s.text_color(content(1.))),
                            )
                            .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                                if event.modifiers().platform {
                                    this.open_terminal_tab(window, cx);
                                } else {
                                    this.create(this.default_kind(), cx);
                                }
                            })),
                    )
                    .child(titlebar_drag_area().flex_1().min_w_0()),
            )
            .into_any()
    }
}
