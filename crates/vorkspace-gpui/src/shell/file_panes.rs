//! File panes share the session pane tree and title strip. Moving a pane
//! transfers its entity, preserving dirty documents, selection and scroll.
use super::*;

impl Shell {
    pub(super) fn independent_file_pane(&mut self, id: i64, cx: &mut Context<Self>) -> Option<i64> {
        let pane = self.take_review_pane(id, cx)?;
        self.workspace.open(pane);
        Some(pane)
    }

    /// On a committed chat drop, leave its file surface in the original
    /// layout slot and give the existing conversation its own movable pane.
    pub(super) fn separate_session_review(
        &mut self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<i64> {
        if self.workspace.reviews.get(&id)?.chat_hidden {
            return None;
        }
        let pane = self.take_review_pane(id, cx)?;
        self.workspace.swap_session(id, pane);
        self.workspace.open(id);
        Some(pane)
    }

    /// A chat can be dropped on its attached file, but never on itself.
    pub(super) fn session_review_hit(&self, id: i64, position: Point<Pixels>) -> Option<Edge> {
        let state = self.workspace.reviews.get(&id)?;
        if state.chat_hidden {
            return None;
        }
        let bounds = self.split_bounds.get(&id)?.get()?;
        let width = bounds.size.width.as_f32();
        let rect = Rect {
            x: width * (1. - state.ratio),
            y: 0.,
            w: width * state.ratio,
            h: bounds.size.height.as_f32(),
        };
        let x = (position.x - bounds.left()).as_f32();
        let y = (position.y - bounds.top()).as_f32();
        (bounds.contains(&position) && x >= rect.x)
            .then(|| vorkspace_model::pane_layout::edge_from_point(x, y, rect))
    }

    fn take_review_pane(&mut self, id: i64, cx: &mut Context<Self>) -> Option<i64> {
        if self.workspace.review_owners.contains_key(&id)
            || self
                .workspace
                .reviews
                .get(&id)
                .is_some_and(|r| r.chat_hidden)
        {
            return Some(id);
        }
        let review = self.reviews.remove(&id)?;
        let pane = self
            .workspace
            .review_owners
            .keys()
            .copied()
            .min()
            .unwrap_or(0)
            - 1;
        self.workspace.review_owners.insert(pane, id);
        self.workspace.reviews.remove(&id);
        self.reviews.insert(pane, review.clone());
        self.review_slide = None;
        review.update(cx, |r, cx| {
            r.state.chat_hidden = true;
            r.set_deferred(false, cx);
            cx.emit(crate::review::ReviewEvent::Changed);
            cx.notify();
        });
        self.workspace
            .reviews
            .insert(pane, review.read(cx).state.clone());
        Some(pane)
    }

    pub(super) fn open_file_pane(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let owner = self.workspace.session_for_pane(id);
        if self.deleting.contains(&owner) {
            return;
        }
        let Some(session) = self.ensure_session(owner, cx) else {
            return;
        };
        self.workspace.project = session.read(cx).summary.cwd.clone();
        if let Some(bots) = &self.bots {
            bots.update(cx, |m, cx| m.close(cx));
        }
        self.navigation_revision += 1;
        self.leave_app_views(cx);
        self.close_preview(cx);
        self.workspace.open(id);
        self.workspace
            .project_focus
            .insert(self.workspace.project.clone(), id);
        self.record_worktree_session(id, cx);
        self.history.record(id);
        self.review_focused = true;
        if let Some(review) = self.review_for(id, cx) {
            review.update(cx, |r, cx| r.focus_active(window, cx));
        }
        self.remember_title_tab(TitleTab::Session(self.workspace.tab_of(id).unwrap()));
        self.reveal_active_tab(cx);
        self.persist(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
