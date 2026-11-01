//! Opening, reusing and closing review panes for a session.
use super::*;

impl Shell {
    pub(super) fn active_review(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Entity<crate::git_tools::review::ReviewPane>> {
        let id = self.workspace.active?;
        self.review_for(id, cx)
    }
    pub(super) fn review_for(
        &mut self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<Entity<crate::git_tools::review::ReviewPane>> {
        if let Some(review) = self.reviews.get(&id) {
            return Some(review.clone());
        }
        let state = self.workspace.reviews.get(&id)?.clone();
        if state.tabs.is_empty() {
            self.workspace.reviews.remove(&id);
            return None;
        }
        Some(self.make_review(id, state, cx))
    }
    pub(super) fn make_review(
        &mut self,
        id: i64,
        state: crate::git_tools::review::ReviewState,
        cx: &mut Context<Self>,
    ) -> Entity<crate::git_tools::review::ReviewPane> {
        use crate::git_tools::review::ReviewEvent;
        let git = self.git.clone();
        let review = cx.new(|cx| {
            let mut pane = crate::git_tools::review::ReviewPane::new(git, state, cx);
            pane.pane_draggable = true;
            pane
        });
        self.observations.push(cx.subscribe(
            &review,
            move |this, review, event: &ReviewEvent, cx| {
                let Some(id) = this
                    .reviews
                    .iter()
                    .find_map(|(id, pane)| (pane == &review).then_some(*id))
                else {
                    return;
                };
                match event {
                    ReviewEvent::WordWrap(wrap) => {
                        this.workspace.preferences.file_word_wrap = *wrap;
                        this.apply_preferences(cx);
                        this.sync_settings(cx);
                        this.persist(cx);
                    }
                    ReviewEvent::Changed => {
                        let state = review.read(cx).state.clone();
                        // A closed last tab never comes back as an empty pane.
                        if state.tabs.is_empty() {
                            return;
                        }
                        this.workspace.reviews.insert(id, state);
                        this.persist(cx);
                    }
                    ReviewEvent::Comment(text) => {
                        let owner = this.workspace.session_for_pane(id);
                        this.add_to_chat(owner, text, cx);
                    }
                    ReviewEvent::Empty => this.close_review(id, cx),
                    ReviewEvent::DragPane(position) => {
                        this.tab_drag = None;
                        this.pane_drag = Some(PaneDrag {
                            source: DragSource::Review(id),
                            start: *position,
                            position: *position,
                            active: false,
                            over: None,
                            insert: None,
                        });
                    }
                }
                cx.notify();
            },
        ));
        self.reviews.insert(id, review.clone());
        review
    }
    /// openChangesTab / openCommitTab in the active tab; the pane slides
    /// in beside the chat when it is new.
    pub(super) fn open_review(
        &mut self,
        tab: crate::git_tools::review::ReviewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_review_with_preview(tab, None, window, cx);
    }
    /// Explorer search previews reuse a clean file tab; explicit opens keep it.
    pub(super) fn open_review_with_preview(
        &mut self,
        tab: crate::git_tools::review::ReviewTab,
        pin: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let crate::git_tools::review::ReviewTab::File { cwd, path } = &tab {
            let recent = self.workspace.recent_files.entry(cwd.clone()).or_default();
            recent.retain(|old| old != path);
            recent.insert(0, path.clone());
            recent.truncate(30);
        }
        // Closing a title tab retains its pane, like a conversation tab.
        // Reopening one of its files must recover the existing dirty editor.
        let retained = self.workspace.review_owners.keys().copied().find(|pane| {
            self.workspace.tab_of(*pane).is_none()
                && self
                    .sessions
                    .get(&self.workspace.session_for_pane(*pane))
                    .is_some_and(|s| s.read(cx).summary.cwd == self.workspace.project)
                && self.in_worktree_scope(*pane, cx)
                && self
                    .workspace
                    .reviews
                    .get(pane)
                    .is_some_and(|r| r.tabs.iter().any(|existing| existing.same_tab(&tab)))
        });
        if let Some(pane) = retained {
            self.open_file_pane(pane, window, cx);
        }
        let Some(id) = self.workspace.active else {
            return;
        };
        let review = match self.review_for(id, cx) {
            Some(review) => review,
            None => {
                let review = self.make_review(id, Default::default(), cx);
                let ratio = review.read(cx).state.ratio;
                let distance = self.split_width() * ratio;
                let slide = match self.review_slide.take() {
                    Some((sid, slide, _)) if sid == id => {
                        slide.retarget(ratio, vorkspace_ui::clock::Instant::now())
                    }
                    _ => PaneSlide::new(0., ratio, distance),
                };
                self.review_slide = Some((id, slide, review.clone()));
                review
            }
        };
        let entering = self
            .review_slide
            .as_ref()
            .is_some_and(|(sid, slide, _)| *sid == id && slide.opening());
        review.update(cx, |r, cx| {
            r.set_deferred(entering, cx);
            if let Some(pin) = pin {
                r.open_file(tab, pin, cx);
            } else {
                r.open(tab, cx);
            }
            r.set_visible(true, cx);
        });
        self.review_focused = true;
        review.update(cx, |r, cx| r.focus_active(window, cx));
        self.fit_sidebar(cx);
        cx.notify();
    }
    /// The pane's last tab closed: it slides out, and a tab without its
    /// chat closes with it.
    pub(super) fn close_review(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(review) = self.reviews.remove(&id) else {
            return;
        };
        review.update(cx, |r, cx| r.cache_closed(cx));
        let chat_hidden = review.read(cx).state.chat_hidden;
        let ratio = review.read(cx).state.ratio;
        self.workspace.reviews.remove(&id);
        if self.workspace.active == Some(id) {
            self.review_focused = false;
            if !chat_hidden {
                // The closing pane stays mounted for its slide-out. Move focus
                // before it disappears, rather than leaving keys in its editor.
                self.pending_composer_focus = Some(self.workspace.session_for_pane(id));
            }
        }
        if chat_hidden {
            // The event has no Window; use the title tabs' close path once
            // it is available, including project and focus restoration.
            cx.spawn(async move |this, cx| {
                let _ = this.update_in(cx, |this, window, cx| {
                    let Some(tab) = this.workspace.tab_of(id) else {
                        return;
                    };
                    if this.reviews.contains_key(&id) {
                        return;
                    }
                    if this.workspace.layout(tab).is_split() {
                        this.close_pane(id, window, cx);
                    } else {
                        this.close_tabs(&[id], None, window, cx);
                    }
                });
            })
            .detach();
        } else if self.workspace.active == Some(id) {
            let distance = self.split_width() * ratio;
            let slide = match self.review_slide.take() {
                Some((sid, slide, _)) if sid == id => {
                    slide.retarget(0., vorkspace_ui::clock::Instant::now())
                }
                _ => PaneSlide::new(ratio, 0., distance),
            };
            self.review_slide = Some((id, slide, review));
        }
        self.fit_sidebar(cx);
        self.persist(cx);
        cx.notify();
    }
    pub(super) fn drag_review(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.review_drag {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.review_drag = false;
            self.persist(cx);
            return;
        }
        let bounds = self
            .workspace
            .active
            .and_then(|id| self.split_bounds.get(&id))
            .and_then(AnchorSlot::get);
        let (Some(bounds), Some(review)) = (bounds, self.active_review(cx)) else {
            return;
        };
        let width = bounds.size.width.as_f32().max(1.);
        let ratio = (1. - (event.position.x - bounds.left()).as_f32() / width).clamp(0.15, 0.85);
        review.update(cx, |r, cx| {
            r.state.ratio = ratio;
            cx.notify();
        });
        cx.notify();
    }
}
