//! Sidebar session rows, their hover previews and keyboard stepping.
use super::*;

impl Shell {
    /// A session doing an orchestrator's task belongs to that run and is not listed alone.
    pub(super) fn orchestration_worker(&self, id: i64) -> bool {
        self.orchestration_summaries
            .values()
            .any(|s| s.tasks.iter().any(|t| t.session == Some(id)))
    }
    pub(super) fn close_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_open = None;
        self.preview_close = None;
        if self.session_preview.take().is_some() {
            self.sidebar_frame.update(cx, |_, cx| cx.notify());
        }
    }
    pub(super) fn schedule_preview(&mut self, id: i64, anchor: AnchorSlot, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            return;
        }
        let settle = std::time::Duration::from_millis(180);
        let delay = self
            .sidebar_last_scroll
            .map(|last| settle.saturating_sub(last.elapsed()))
            .unwrap_or_default()
            + std::time::Duration::from_millis(120);
        self.preview_open = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let Ok(Some(store)) = this.update_in(cx, |this, window, cx| {
                if this.session_hover != Some(id)
                    || this.menu.is_some()
                    || !this.session_summaries.contains_key(&id)
                    || !anchor
                        .get()
                        .is_some_and(|bounds| bounds.contains(&window.mouse_position()))
                {
                    return None;
                }
                this.session_preview = Some(SessionPreview {
                    id,
                    data: None,
                    anchor,
                    surface: AnchorSlot::default(),
                });
                this.sidebar_frame.update(cx, |_, cx| cx.notify());
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
                    .session_preview
                    .as_mut()
                    .filter(|preview| preview.id == id)
                {
                    preview.data = data;
                    this.sidebar_frame.update(cx, |_, cx| cx.notify());
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
        let previous = self.session_hover;
        if hovered {
            self.session_hover = Some(id);
        } else if self.session_hover == Some(id) {
            self.session_hover = None;
        }
        if previous != self.session_hover {
            self.sidebar_frame.update(cx, |_, cx| cx.notify());
        }
        if hovered && compact {
            self.preview_close = None;
            if self
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
            if self.session_hover.is_none()
                && self
                    .session_preview
                    .as_ref()
                    .is_none_or(|preview| preview.id != id)
            {
                self.preview_open = None;
            }
            if self
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
        if self.session_preview.as_ref().is_some_and(|preview| {
            preview
                .surface
                .get()
                .is_some_and(|bounds| bounds.contains(&window.mouse_position()))
        }) {
            self.preview_close = None;
            return;
        }
        self.preview_close = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            let _ = this.update(cx, |this, cx| this.close_preview(cx));
        }));
    }

    /// The project's sessions as the sidebar lists them, top to bottom.
    /// Every session the rail can list: the vorkspace's projects, through
    /// the session filter and title search, newest first. The rail shows a
    /// window of these; selection and menus act on the same set.
    pub(super) fn sidebar_session_ids(&self, cx: &App) -> Vec<i64> {
        let start = self
            .workspace
            .session_filters
            .time
            .start(vorkspace_core::time::now_ms());
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
                (s.summary.has_messages || s.busy)
                    && projects.contains(&vorkspace_core::files::path_key(&s.summary.cwd))
                    && !self
                        .bots
                        .as_ref()
                        .is_some_and(|m| m.read(cx).hidden_session(id))
                    && !self.orchestration_worker(id)
                    && self.filter_session(id, start, cx)
                    && (self.query.is_empty()
                        || s.display_title().to_lowercase().contains(&self.query))
            })
            .collect();
        ids.sort_by_key(|id| {
            std::cmp::Reverse((self.session_metadata(*id, cx).summary.updated_at, *id))
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
