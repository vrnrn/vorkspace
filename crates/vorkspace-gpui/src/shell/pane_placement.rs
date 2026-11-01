//! Pane admission, splitting, focus movement, drag placement and sash resizing.
use super::*;

impl Shell {
    /// A click anywhere in a pane focuses it (PaneTree's onFocus).
    pub(super) fn focus_pane(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.review_owners.contains_key(&id) {
            self.open_file_pane(id, window, cx);
            return;
        }
        if self.workspace.active == Some(id) {
            return;
        }
        self.navigation_revision += 1;
        self.history.record(id);
        self.workspace.open(id);
        if let Some(view) = self.sessions.get(&id) {
            self.workspace.project = view.read(cx).summary.cwd.clone();
            self.workspace
                .project_focus
                .insert(self.workspace.project.clone(), id);
            view.update(cx, |v, cx| v.load(cx));
            self.record_worktree_session(id, cx);
        }
        if !self.review_focused {
            self.focus_active(window, cx);
        }
        self.persist(cx);
        cx.notify();
    }
    /// Close one pane of a split tab; the tab's only pane closes the tab.
    pub(super) fn close_pane(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.workspace.active;
        let before = self.active_leaves();
        if !self.workspace.close_pane(id) {
            self.close(id, window, cx);
            return;
        }
        self.history
            .prune(&self.workspace.open_sessions(), self.workspace.active);
        self.navigation_revision += 1;
        self.begin_pane_motion(before);
        self.fit_sidebar(cx);
        if self.workspace.active != active {
            if let Some(active) = self.workspace.active {
                self.open(active, window, cx);
            }
        } else {
            self.persist(cx);
            cx.notify();
        }
    }
    pub(super) fn pane_available_width(&self) -> f32 {
        let sidebar = if self.workspace.sidebar_visible {
            self.workspace.right_panel_width
        } else {
            0.
        };
        let dock = self
            .workspace
            .docks
            .get(&self.workspace.project)
            .filter(|dock| dock.open && !dock.side.vertical())
            .map_or(0., |dock| dock.size);
        let expected =
            (self.viewport_width - self.classic_rail_width() - self.right_drawn - sidebar - dock)
                .max(0.);
        self.pane_viewport
            .get()
            .map_or(expected, |bounds| bounds.size.width.as_f32().min(expected))
    }

    /// `fit_sidebar` automatically hides the session sidebar when a split
    /// needs its room and restores it when it fits again.
    pub(super) fn pane_admission_width(&self) -> f32 {
        self.pane_available_width()
            + if self.workspace.sidebar_visible {
                self.workspace.right_panel_width
            } else {
                0.
            }
    }

    pub(super) fn pane_layout_allowed(&self, before: &PaneNode, after: &PaneNode) -> bool {
        let session = |id: i64| id >= 0 && !self.workspace.review_owners.contains_key(&id);
        let old_stack = before.max_session_stack(session);
        let new_stack = after.max_session_stack(session);
        let stack_ok = new_stack <= MAX_SESSION_STACK
            || (old_stack > MAX_SESSION_STACK && new_stack < old_stack);
        let available = self.pane_admission_width();
        let old_width = before.minimum_width(MIN_CHAT_WIDTH);
        let new_width = after.minimum_width(MIN_CHAT_WIDTH);
        stack_ok && new_width <= available.max(old_width) + 0.5
    }

    pub(super) fn fit_active_pane_width(&mut self) {
        let width = self.pane_available_width();
        if let Some(tab) = self.workspace.active_tab()
            && let Some(layout) = self.workspace.layouts.get_mut(&tab)
        {
            let width = width.max(layout.minimum_width(MIN_CHAT_WIDTH));
            layout.fit_width(width, MIN_CHAT_WIDTH);
        }
    }

    pub(super) fn placement_allowed(&self, source: DragSource, target: i64, edge: Edge) -> bool {
        let Some(tab) = self.workspace.tab_of(target) else {
            return false;
        };
        let before = self.workspace.layout(tab);
        let id = source.session();
        let after = match source {
            DragSource::Tab(source_tab) => {
                if source_tab == tab {
                    return false;
                }
                before.place_layout(self.workspace.layout(source_tab), target, edge)
            }
            DragSource::Review(owner)
                if self.workspace.review_owners.contains_key(&owner)
                    || self
                        .workspace
                        .reviews
                        .get(&owner)
                        .is_some_and(|r| r.chat_hidden) =>
            {
                before.place_pane(owner, target, edge)
            }
            DragSource::Review(_) => before.place_pane(i64::MIN, target, edge),
            DragSource::Pane(owner)
                if self
                    .workspace
                    .reviews
                    .get(&owner)
                    .is_some_and(|r| !r.chat_hidden) =>
            {
                let base = before.replace_leaf(owner, i64::MIN);
                let target = if target == owner { i64::MIN } else { target };
                base.place_pane(owner, target, edge)
            }
            _ => before.place_pane(id, target, edge),
        };
        let adds_pane = after.leaf_ids().len() > before.leaf_ids().len();
        if matches!(edge, Edge::Left | Edge::Right)
            && adds_pane
            && after.minimum_width(MIN_CHAT_WIDTH) > self.pane_admission_width() + 0.5
        {
            return false;
        }
        self.pane_layout_allowed(&before, &after)
    }

    pub(super) fn split_allowed(
        &self,
        before: &PaneNode,
        target: i64,
        dir: SplitDir,
        id: i64,
    ) -> bool {
        let after = before.split_pane(target, dir, id);
        (dir != SplitDir::Right
            || after.minimum_width(MIN_CHAT_WIDTH) <= self.pane_admission_width() + 0.5)
            && self.pane_layout_allowed(before, &after)
    }

    pub(super) fn reject_pane_placement(&mut self, cx: &mut Context<Self>) {
        self.error = Some(PANE_LIMIT_MESSAGE.into());
        cx.notify();
    }
    /// cmd-d / shift-cmd-d: a new session beside the focused pane, with the
    /// focused session's provider.
    pub(super) fn split_session(&mut self, dir: SplitDir, cx: &mut Context<Self>) {
        if self.app_view_open() || self.workspace.active.is_none() {
            return;
        }
        let focused = self.workspace.active.unwrap();
        let Some(tab) = self.workspace.tab_of(focused) else {
            return;
        };
        let before = self.workspace.layout(tab);
        if !self.split_allowed(&before, focused, dir, i64::MAX) {
            self.reject_pane_placement(cx);
            return;
        }
        let kind = self
            .workspace
            .active
            .and_then(|id| self.sessions.get(&id))
            .map_or(Kind::Codex, |s| Kind::from_id(&s.read(cx).summary.provider));
        let navigation_revision = self.navigation_revision;
        let target = self.workspace.active;
        let store = self.store.clone();
        let cwd = self.workspace.project.clone();
        let worktree = target.and_then(|id| {
            let path = self.session_working_copy(id, cx);
            (path != cwd).then(|| crate::worktree_switcher::WorktreeFocus {
                path: path.into(),
                branch: self.worktree_focus.get(&cwd).and_then(|f| f.branch.clone()),
            })
        });
        let worktree_cwd = worktree.as_ref().map(|f| f.path.clone());
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            let summary = store.create(kind.id(), &cwd)?;
            if worktree_cwd.is_some() {
                store.save_agent_settings(
                    summary.id,
                    &vorkspace_core::agent::Settings {
                        worktree_cwd,
                        ..Default::default()
                    },
                )?;
            }
            Ok::<_, String>(summary)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(summary) => this.finish_split_creation(
                    summary,
                    worktree.as_ref(),
                    (target, navigation_revision, dir),
                    window,
                    cx,
                ),
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn finish_split_creation(
        &mut self,
        summary: Session,
        worktree: Option<&crate::worktree_switcher::WorktreeFocus>,
        request: (Option<i64>, u64, SplitDir),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (target, navigation_revision, dir) = request;
        let id = summary.id;
        self.add(summary, cx);
        self.seed_worktree(id, worktree, cx);
        self.order.retain(|s| *s != id);
        self.order.insert(0, id);
        if self.navigation_revision != navigation_revision || self.workspace.active != target {
            // The user navigated while creation was in flight. Keep the new
            // blank session reachable without stealing the newer focus.
            let active = self.workspace.active;
            self.workspace.open(id);
            self.workspace.active = active;
            self.persist(cx);
            self.invalidate_chrome(cx);
            cx.notify();
            return;
        }
        let Some(target) = target else {
            return;
        };
        let Some(tab) = self.workspace.tab_of(target) else {
            return;
        };
        let layout = self.workspace.layout(tab);
        if !self.split_allowed(&layout, target, dir, id) {
            self.workspace.open(id);
            self.activate_session(id, window, cx);
            self.reject_pane_placement(cx);
            return;
        }
        let before = self.active_leaves();
        self.workspace.split(id, dir);
        self.fit_active_pane_width();
        self.begin_pane_motion(before);
        self.fit_sidebar(cx);
        self.activate_session(id, window, cx);
    }
    pub(super) fn focus_neighbor(
        &mut self,
        dir: Edge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.workspace.neighbor(dir) {
            self.review_focused = false;
            self.open(id, window, cx);
        }
    }
    pub(super) fn active_leaves(&self) -> Vec<LayoutLeaf> {
        self.workspace
            .active_tab()
            .map(|tab| self.workspace.layout(tab).leaves())
            .unwrap_or_default()
    }
    /// Animate the active tab from `before` to its current tree. New panes
    /// enter from the outer edge they sit on, or open from their middle.
    pub(super) fn begin_pane_motion(&mut self, before: Vec<LayoutLeaf>) {
        let Some(tab) = self.workspace.active_tab() else {
            return;
        };
        let after = self.workspace.layout(tab).leaves();
        if (after.len() < 2 && before.len() < 2) || before.is_empty() {
            self.pane_motion = None;
            return;
        }
        let (width, height) = self.pane_bounds.get().map_or((800., 600.), |b| {
            (b.size.width.as_f32(), b.size.height.as_f32())
        });
        let mut from = HashMap::new();
        let mut entering = HashMap::new();
        let mut travel: f32 = 0.;
        for leaf in &after {
            let start = match before.iter().find(|b| b.id == leaf.id) {
                Some(old) => old.rect,
                None => {
                    let (edge, start) = enter_from(leaf);
                    entering.insert(leaf.id, edge);
                    start
                }
            };
            travel = travel.max(
                ((start.w - leaf.rect.w).abs() * width)
                    .max((start.h - leaf.rect.h).abs() * height)
                    .max((start.x - leaf.rect.x).abs() * width)
                    .max((start.y - leaf.rect.y).abs() * height),
            );
            from.insert(leaf.id, start);
        }
        // A closed pane slides back out the way a new one comes in. One that
        // moved to another tab is shown there instead.
        let open = self.workspace.open_sessions();
        let leaving: Vec<_> = before
            .iter()
            .filter(|old| {
                !after.iter().any(|leaf| leaf.id == old.id)
                    && !open.contains(&old.id)
                    && self.sessions.contains_key(&old.id)
            })
            .map(|old| {
                let (edge, to) = enter_from(old);
                travel = travel.max((old.rect.w * width).max(old.rect.h * height) * 0.5);
                (*old, edge, to)
            })
            .collect();
        if travel < 1. {
            self.pane_motion = None;
            return;
        }
        let header = match (before.len() > 1, after.len() > 1) {
            (false, true) => Some(true),
            (true, false) => Some(false),
            _ => None,
        };
        self.pane_motion = Some(PaneMotion {
            tab,
            from,
            entering,
            leaving,
            header,
            slide: PaneSlide::new(0., 1., travel),
        });
    }
    /// The pane under a point and the edge it would take, in the active tab.
    pub(super) fn pane_hit(&self, position: Point<Pixels>) -> Option<(i64, Edge)> {
        if self.app_view_open() {
            return None;
        }
        if self
            .pane_viewport
            .get()
            .is_none_or(|bounds| !bounds.contains(&position))
        {
            return None;
        }
        let bounds = self.pane_bounds.get()?;
        if !bounds.contains(&position) {
            return None;
        }
        let tab = self.workspace.active_tab()?;
        let (w, h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
        let (x, y) = (
            (position.x - bounds.left()).as_f32(),
            (position.y - bounds.top()).as_f32(),
        );
        self.workspace
            .layout(tab)
            .leaves()
            .into_iter()
            .find_map(|leaf| {
                let r = Rect {
                    x: leaf.rect.x * w,
                    y: leaf.rect.y * h,
                    w: leaf.rect.w * w,
                    h: leaf.rect.h * h,
                };
                (x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h).then(|| {
                    (
                        leaf.id,
                        vorkspace_model::pane_layout::edge_from_point(x, y, r),
                    )
                })
            })
    }
    /// titleTabDropFromPoint: the strip slot nearest the pointer.
    pub(super) fn tab_insert_at(&self, position: Point<Pixels>, cx: &App) -> Option<usize> {
        let strip = self.tab_scroll.bounds();
        if !strip.contains(&position) {
            return None;
        }
        let count = self.visible_tabs(cx).len();
        let bounds: Vec<_> = (0..count)
            .filter_map(|ix| self.tab_scroll.bounds_for_item(ix))
            .collect();
        if bounds.len() != count {
            return Some(count);
        }
        Some(
            bounds
                .iter()
                .position(|b| position.x < b.center().x)
                .unwrap_or(count),
        )
    }
    pub(super) fn move_pane_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.pane_drag.as_ref() else {
            return;
        };
        if matches!(drag.source, DragSource::Tab(_)) {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.cancel_pane_drag(cx);
            return;
        }
        let source = drag.source;
        let start = drag.start;
        let delta = event.position - start;
        if !drag.active && delta.x.as_f32().hypot(delta.y.as_f32()) < 5. {
            return;
        }
        let insert = match source {
            DragSource::Pane(_) | DragSource::Review(_) => self.tab_insert_at(event.position, cx),
            _ => None,
        };
        let over = if insert.is_some() {
            None
        } else {
            self.pane_hit(event.position)
                .and_then(|(id, edge)| {
                    if id != source.session()
                        || matches!(source, DragSource::Review(id) if self.workspace.reviews.get(&id).is_some_and(|r| !r.chat_hidden))
                    {
                        Some((id, edge))
                    } else if matches!(source, DragSource::Pane(_)) {
                        self.session_review_hit(id, event.position).map(|edge| (id, edge))
                    } else {
                        None
                    }
                })
        };
        let drag = self.pane_drag.as_mut().unwrap();
        if !drag.active && matches!(source, DragSource::Session(_)) {
            self.skip_session_click = true;
        }
        drag.active = true;
        drag.position = event.position;
        drag.over = over;
        if drag.insert != insert {
            drag.insert = insert;
            self.tabs_frame.update(cx, |_, cx| cx.notify());
        }
        let drop = matches!(source, DragSource::Session(_))
            .then(|| self.sidebar_drop_at(event.position, source.session(), cx))
            .flatten();
        if self.sidebar_drop != drop {
            self.sidebar_drop = drop;
            self.notify_sidebar(cx);
        }
        cx.notify();
    }
    pub(super) fn cancel_pane_drag(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_drop.take().is_some() {
            self.notify_sidebar(cx);
        }
        if let Some(drag) = self.pane_drag.take() {
            if drag.insert.is_some() {
                self.tabs_frame.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        }
    }
    pub(super) fn finish_pane_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.pane_drag.take() else {
            return;
        };
        self.tabs_frame.update(cx, |_, cx| cx.notify());
        cx.notify();
        if !drag.active {
            return;
        }
        if let Some(target) = self.sidebar_drop.take() {
            self.apply_sidebar_drop(drag.source.session(), target, window, cx);
            return;
        }
        if let Some((target, edge)) = drag.over
            && drag.insert.is_none()
            && !self.placement_allowed(drag.source, target, edge)
        {
            self.reject_pane_placement(cx);
            return;
        }
        let mut before = self.active_leaves();
        let id = drag.source.session();
        let mut over = drag.over;
        if matches!(drag.source, DragSource::Pane(_))
            && (over.is_some() || drag.insert.is_some())
            && let Some(review) = self.separate_session_review(id, cx)
        {
            // The file keeps the old pane's place while the chat moves.
            for leaf in &mut before {
                if leaf.id == id {
                    leaf.id = review;
                }
            }
            if let Some((target, _)) = &mut over
                && *target == id
            {
                *target = review;
            }
        }
        match (drag.source, over, drag.insert) {
            (DragSource::Review(_), over, insert) if over.is_some() || insert.is_some() => {
                let Some(id) = self.independent_file_pane(id, cx) else {
                    return;
                };
                if let Some(to) = insert {
                    self.workspace.detach(id, self.workspace.tabs.len());
                    self.reorder_title_tab(&TitleTab::Session(id), to, cx);
                    self.pane_motion = None;
                } else if let Some((target, edge)) = over {
                    self.workspace.place(id, target, edge);
                    self.begin_pane_motion(before);
                }
            }
            (DragSource::Pane(_), _, Some(to)) => {
                let tabs = self.visible_tabs(cx);
                let session_index = tabs
                    .get(to)
                    .and_then(TitleTab::session)
                    .and_then(|next| self.workspace.tabs.iter().position(|id| *id == next))
                    .unwrap_or(self.workspace.tabs.len());
                self.workspace.detach(id, session_index);
                self.reorder_title_tab(&TitleTab::Session(id), to, cx);
                self.pane_motion = None;
            }
            (DragSource::Tab(tab), Some((target, edge)), _) => {
                self.workspace.place_tab(tab, target, edge);
                self.begin_pane_motion(before);
            }
            (DragSource::Pane(_) | DragSource::Session(_), Some((target, edge)), _) => {
                self.workspace.place(id, target, edge);
                self.begin_pane_motion(before);
            }
            _ => return,
        }
        self.fit_active_pane_width();
        self.history
            .prune(&self.workspace.open_sessions(), self.workspace.active);
        self.fit_sidebar(cx);
        if let Some(active) = self.workspace.active {
            self.open(active, window, cx);
        }
    }
    pub(super) fn drag_sash(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(sash) = self.pane_sash.as_ref() else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.release_resizes(cx);
            return;
        }
        let Some(target) = self.sash_target() else {
            return;
        };
        let Some(bounds) = self.pane_bounds.get() else {
            return;
        };
        let g = sash.group;
        let (at, origin) = if sash.dir == SplitDir::Right {
            let origin = bounds.left().as_f32() + g.x * bounds.size.width.as_f32();
            (event.position.x.as_f32(), origin)
        } else {
            let origin = bounds.top().as_f32() + g.y * bounds.size.height.as_f32();
            (event.position.y.as_f32(), origin)
        };
        let ResizeTarget::Sash { span, .. } = target else {
            return;
        };
        self.drag_resize(target, (at - origin) / span.max(1.), cx);
    }
    pub(super) fn sash_target(&self) -> Option<ResizeTarget> {
        let sash = self.pane_sash.as_ref()?;
        let bounds = self.pane_bounds.get()?;
        let span = if sash.dir == SplitDir::Right {
            sash.group.w * bounds.size.width.as_f32()
        } else {
            sash.group.h * bounds.size.height.as_f32()
        };
        Some(ResizeTarget::Sash {
            tab: sash.tab,
            path: sash.path.clone(),
            index: sash.index,
            span,
        })
    }
    /// The target's current size, the range it rests in, and pixels per unit.
    pub(super) fn resize_extent(&self, target: &ResizeTarget) -> Option<(f32, f32, f32, f32)> {
        Some(match target {
            ResizeTarget::ProjectRail => {
                let min = self.open_rail_floor().clamp(180., 360.);
                (self.workspace.project_rail_width.max(min), min, 360., 1.)
            }
            ResizeTarget::Sidebar | ResizeTarget::RightPanel => {
                let (min, max) = (crate::right_panel::WIDTH_MIN, crate::right_panel::WIDTH_MAX);
                let max = max.min((self.viewport_width * 0.5).floor()).max(min);
                (self.workspace.right_panel_width, min, max, 1.)
            }
            ResizeTarget::Dock(project) => {
                let dock = self.workspace.docks.get(project)?;
                let (min, max) = dock.side.limits(self.viewport());
                (dock.size, min, max, 1.)
            }
            ResizeTarget::Sash {
                tab,
                path,
                index,
                span,
            } => {
                let (at, min, max) = self
                    .workspace
                    .layout(*tab)
                    .horizontal_sash_range(path, *index, *span, MIN_CHAT_WIDTH)
                    .or_else(|| self.workspace.sash_range(*tab, path, *index))?;
                (at, min, max, span.max(1.))
            }
        })
    }
    pub(super) fn set_resize(&mut self, target: &ResizeTarget, value: f32) {
        match target {
            ResizeTarget::ProjectRail => self.workspace.project_rail_width = value,
            ResizeTarget::Sidebar | ResizeTarget::RightPanel => {
                self.workspace.right_panel_width = value
            }
            ResizeTarget::Dock(project) => {
                if let Some(dock) = self.workspace.docks.get_mut(project) {
                    dock.size = value;
                }
            }
            ResizeTarget::Sash {
                tab,
                path,
                index,
                span,
            } => {
                let value = self
                    .workspace
                    .layout(*tab)
                    .horizontal_sash_range(path, *index, *span, MIN_CHAT_WIDTH)
                    .map_or(value, |(_, min, max)| value.clamp(min, max));
                self.workspace.place_sash(*tab, path, *index, value)
            }
        }
    }
    /// Follows the pointer to `value`, stopping at the target's limits. The
    /// moment the drag hits one, the edge knocks a little past it and springs
    /// back, harder the faster it arrived.
    pub(super) fn drag_resize(&mut self, target: ResizeTarget, value: f32, cx: &mut Context<Self>) {
        if let Some((settling, settle)) = self.resize_settle.take() {
            if settling == target {
                self.resize_settle = Some((settling, settle));
            } else {
                self.set_resize(&settling, settle.target());
            }
        }
        let Some((_, min, max, scale)) = self.resize_extent(&target) else {
            return;
        };
        let clamped = value.clamp(min, max);
        let pressing = (value - clamped).abs() * scale > 0.5;
        let to = if matches!(target, ResizeTarget::Sash { .. }) {
            clamped
        } else {
            clamped.round()
        };
        if pressing && !self.resize_pinned {
            let past = (value - clamped).abs() * scale;
            let overshoot = (past * 0.4).clamp(3., 8.) * (value - clamped).signum() / scale;
            self.resize_settle = Some((target.clone(), ResizeSettle::hit(to, overshoot, scale)));
        } else if !pressing {
            self.resize_settle = None;
        }
        self.resize_pinned = pressing;
        if self.resize_settle.is_none() {
            self.set_resize(&target, to);
        }
        cx.notify();
    }
    /// Ends every resize in progress; a bounce under way plays out.
    pub(super) fn release_resizes(&mut self, cx: &mut Context<Self>) -> bool {
        let any = [
            self.pane_sash.take().is_some(),
            self.resize.take().is_some(),
            self.project_rail_drag.take().is_some(),
            self.right_resize.take().is_some(),
            self.dock_resize.take().is_some(),
        ]
        .contains(&true);
        self.resize_pinned = false;
        if any {
            self.persist_settled(cx);
            cx.notify();
        }
        any
    }
    /// Saves the layout with any bouncing resize at its resting size.
    pub(super) fn persist_settled(&mut self, cx: &mut Context<Self>) {
        let Some((target, settle)) = self.resize_settle.take() else {
            self.persist(cx);
            return;
        };
        let shown = self.resize_extent(&target).map(|extent| extent.0);
        self.set_resize(&target, settle.target());
        self.persist(cx);
        if let Some(shown) = shown {
            self.set_resize(&target, shown);
        }
        self.resize_settle = Some((target, settle));
    }
    pub(super) fn tick_resize_settle(&mut self, window: &mut Window) {
        let Some((target, settle)) = &self.resize_settle else {
            return;
        };
        let (value, done) = settle.at(vorkspace_ui::clock::Instant::now());
        let target = target.clone();
        self.set_resize(&target, value);
        if done {
            self.resize_settle = None;
        } else {
            window.request_animation_frame();
        }
    }
}
