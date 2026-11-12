//! Sidebar session rows, their hover previews and keyboard stepping.
use super::*;

impl Shell {
    /// A session doing an orchestrator's task belongs to that run and is not listed alone.
    pub(super) fn orchestration_worker(&self, id: i64) -> bool {
        self.sidebar
            .orchestration_summaries
            .values()
            .any(|s| s.tasks.iter().any(|t| t.session == Some(id)))
    }
    pub(super) fn close_preview(&mut self, cx: &mut Context<Self>) {
        self.preview.open = None;
        self.preview.close = None;
        if self.preview.session_preview.take().is_some() {
            self.sidebar_frame.update(cx, |_, cx| cx.notify());
            self.project_rail_frame.update(cx, |_, cx| cx.notify());
            self.invalidate_chrome(cx);
            cx.notify();
        }
    }
    pub(super) fn schedule_preview(&mut self, id: i64, anchor: AnchorSlot, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            return;
        }
        let settle = std::time::Duration::from_millis(180);
        let delay = self
            .preview
            .last_scroll
            .map(|last| settle.saturating_sub(last.elapsed()))
            .unwrap_or_default()
            + std::time::Duration::from_millis(120);
        self.preview.open = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let Ok(Some(store)) = this.update_in(cx, |this, window, cx| {
                if this.preview.session_hover != Some(id)
                    || this.menu.is_some()
                    || !this.has_session(id, cx)
                    || !anchor
                        .get()
                        .is_some_and(|bounds| bounds.contains(&window.mouse_position()))
                {
                    return None;
                }
                this.preview.session_preview = Some(SessionPreview {
                    id,
                    data: None,
                    anchor,
                    surface: AnchorSlot::default(),
                });
                this.sidebar_frame.update(cx, |_, cx| cx.notify());
                this.project_rail_frame.update(cx, |_, cx| cx.notify());
                this.invalidate_chrome(cx);
                cx.notify();
                // An already-open conversation has its own live metadata.
                (!this.sessions.contains_key(&id)).then(|| this.store.clone())
            }) else {
                return;
            };
            let data = cx
                .background_executor()
                .spawn(async move { locked(&store).and_then(|store| store.session_preview(id)) })
                .await
                .ok();
            let _ = this.update(cx, |this, cx| {
                if let Some(preview) = this
                    .preview
                    .session_preview
                    .as_mut()
                    .filter(|preview| preview.id == id)
                {
                    preview.data = data;
                    this.sidebar_frame.update(cx, |_, cx| cx.notify());
                    this.project_rail_frame.update(cx, |_, cx| cx.notify());
                    this.invalidate_chrome(cx);
                    cx.notify();
                }
            });
        }));
    }
    pub(super) fn hover_session(
        &mut self,
        id: i64,
        anchor: AnchorSlot,
        hovered: bool,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.deleting.contains(&id) {
            return;
        }
        let previous = self.preview.session_hover;
        if hovered {
            self.preview.session_hover = Some(id);
        } else if self.preview.session_hover == Some(id) {
            self.preview.session_hover = None;
        }
        if previous != self.preview.session_hover {
            self.sidebar_frame.update(cx, |_, cx| cx.notify());
            self.project_rail_frame.update(cx, |_, cx| cx.notify());
            self.invalidate_chrome(cx);
            cx.notify();
        }
        if hovered && compact {
            self.preview.close = None;
            if self
                .preview
                .session_preview
                .as_ref()
                .is_none_or(|preview| preview.id != id)
            {
                self.close_preview(cx);
                self.schedule_preview(id, anchor, cx);
            }
        } else {
            // A row's leave can arrive after the next row's enter. Do not
            // cancel that newer hover, or metadata loading for an open card.
            if self.preview.session_hover.is_none()
                && self
                    .preview
                    .session_preview
                    .as_ref()
                    .is_none_or(|preview| preview.id != id)
            {
                self.preview.open = None;
            }
            if self
                .preview
                .session_preview
                .as_ref()
                .is_some_and(|preview| preview.id == id)
            {
                self.schedule_preview_close(window, cx);
            }
        }
    }
    pub(super) fn schedule_preview_close(&mut self, window: &Window, cx: &mut Context<Self>) {
        // During the entrance the card can already cover the gap beside its
        // row. The row's leave event must not undo the card's enter event.
        if self
            .preview
            .session_preview
            .as_ref()
            .is_some_and(|preview| {
                preview
                    .surface
                    .get()
                    .is_some_and(|bounds| bounds.contains(&window.mouse_position()))
            })
        {
            self.preview.close = None;
            return;
        }
        self.preview.close = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            let _ = this.update(cx, |this, cx| this.close_preview(cx));
        }));
    }

    pub(super) fn sidebar_session_ids(&self, cx: &App) -> Vec<i64> {
        self.filtered_rail_sessions(&self.session_launchers(cx), cx)
    }

    pub(super) fn session_launchers<'a>(
        &self,
        cx: &'a App,
    ) -> HashMap<i64, &'a vorkspace_core::bot::Bot> {
        let mut launches = HashMap::new();
        if let Some(bots) = &self.bots {
            for bot in &bots.read(cx).roster.bots {
                for launch in &bot.launches {
                    launches.entry(launch.session_id).or_insert(bot);
                }
            }
        }
        launches
    }

    /// Resolve scope and filters once before projecting the visible cards.
    pub(super) fn filtered_rail_sessions(
        &self,
        launches: &HashMap<i64, &vorkspace_core::bot::Bot>,
        cx: &App,
    ) -> Vec<i64> {
        let filters = self.effective_session_filters(cx);
        let start = filters.time.start(vorkspace_core::time::now_ms());
        let bots = self.bots.as_ref().map(|bots| bots.read(cx));
        let bot_sessions: HashSet<i64> = bots
            .into_iter()
            .flat_map(|bots| bots.roster.hidden_sessions.iter().copied())
            .collect();
        let projects: HashSet<String> = self
            .workspace
            .vorkspace_projects()
            .iter()
            .map(|p| vorkspace_core::files::path_key(p))
            .collect();
        let mut ids: Vec<_> = self
            .order
            .iter()
            .copied()
            .filter(|&id| {
                let s = self.session_metadata(id, cx);
                let launcher = launches.get(&id);
                (s.summary.has_messages || s.busy)
                    && projects.contains(&vorkspace_core::files::path_key(&s.summary.cwd))
                    && !bot_sessions.contains(&id)
                    && launcher.is_none_or(|bot| bot.show_started_sessions_in_sidebar)
                    && !self.orchestration_worker(id)
                    && filters.bot.matches(launcher.map(|bot| bot.id.as_str()))
                    && filters.matches(
                        s.summary,
                        vorkspace_core::session_filters::Status {
                            working: s.responding(),
                            needs_approval: s.needs_approval(),
                            done: self.unseen_finished.contains(&id),
                        },
                        start,
                        self.workspace.preferences.show_archived,
                    )
                    && (self.sidebar.query.is_empty()
                        || s.display_title()
                            .to_lowercase()
                            .contains(&self.sidebar.query)
                        || self
                            .sidebar
                            .groups
                            .get(&s.summary.cwd)
                            .is_some_and(|folders| folders.matches(id, &self.sidebar.query)))
            })
            .collect();
        ids.sort_by_cached_key(|id| {
            let s = self.session_metadata(*id, cx);
            std::cmp::Reverse((
                s.responding() || s.needs_approval(),
                s.summary.pinned,
                s.summary.updated_at,
                *id,
            ))
        });
        ids
    }
    /// `Session: Previous/Next` opens the sidebar's neighbour of the active
    /// session; `in_tab` swaps it into the current tab instead.
    pub(super) fn step_session(
        &mut self,
        step: isize,
        in_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu.is_some() {
            return;
        }
        let ids = self.sidebar_navigation_ids(cx);
        let current = self.workspace.active;
        let Some(next) = adjacent(&ids, current.as_ref(), step) else {
            return;
        };
        if Some(next) == current {
            return;
        }
        if in_tab
            && self.workspace.tab_of(next).is_none()
            && let Some(active) = current
            && self.pane_in_scope(next, cx)
            && !self.deleting.contains(&next)
            && self.ensure_session(next, cx).is_some()
        {
            let previous_tab = self.workspace.tab_of(active);
            self.workspace.swap_session(active, next);
            if let Some((from, to)) = previous_tab.zip(self.workspace.tab_of(next)) {
                self.replace_title_tab_key(from, to);
            }
        }
        self.open(next, window, cx);
    }
    /// `Project: Previous/Next` along the project rail.
    pub(super) fn step_project(
        &mut self,
        step: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu.is_some() {
            return;
        }
        let projects = self.workspace.vorkspace_projects();
        if let Some(next) = adjacent(&projects, Some(&self.workspace.project), step)
            && next != self.workspace.project
        {
            self.select_project(next, window, cx);
        }
    }
    pub(super) fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.classic_sidebar(cx)
    }
}
