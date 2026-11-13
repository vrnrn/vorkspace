//! Opening, reusing and closing the workspace's file and review panes.
use super::*;
use crate::workspace::AppPane;

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
        if let Some(review) = self.review.reviews.get(&id) {
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
        let cache = self.review.cache.clone();
        let review = cx.new(|cx| {
            let mut pane = crate::git_tools::review::ReviewPane::new(git, cache, state, cx);
            pane.pane_draggable = true;
            pane
        });
        self.observations.push(cx.subscribe(
            &review,
            move |this, review, event: &ReviewEvent, cx| {
                let Some(id) = this
                    .review
                    .reviews
                    .iter()
                    .find_map(|(id, pane)| (pane == &review).then_some(*id))
                else {
                    if let ReviewEvent::Comment(comment) = event {
                        review.update(cx, |review, cx| {
                            review.complete_comment(
                                comment,
                                Err("This review has closed.".into()),
                                cx,
                            );
                        });
                    }
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
                    ReviewEvent::Comment(comment) => {
                        if !comment.is_current(cx) {
                            return;
                        }
                        let result = this.add_review_comment(id, comment.text(), cx);
                        review.update(cx, |review, cx| {
                            review.complete_comment(comment, result, cx)
                        });
                    }
                    ReviewEvent::Empty => this.close_review(id, cx),
                    ReviewEvent::DragPane(position) => {
                        this.tabs.drag = None;
                        this.panes.drag = Some(PaneDrag {
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
        self.review.reviews.insert(id, review.clone());
        review
    }

    fn add_review_comment(
        &mut self,
        pane: i64,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.workspace.tab_of(pane).is_none() {
            return Err("Reopen the review before adding this comment.".into());
        }
        if let Some(owner) = self.workspace.session_for_pane(pane) {
            return self.add_to_chat(owner, text, cx);
        }
        let bot = self
            .workspace
            .app_panes
            .get(&pane)
            .and_then(|pane| pane.review_bot.as_deref())
            .ok_or("Open this file from a chat to add a comment there.")?;
        if !self.workspace.preferences.bots {
            return Err("Enable Bots in Settings, then try again.".into());
        }
        let manager = self
            .bots
            .clone()
            .ok_or("The originating Bot is unavailable.")?;
        let chat = manager.update(cx, |manager, cx| {
            let actor = manager
                .roster
                .get(bot)
                .ok_or("The originating Bot is unavailable. Copy the comment to another chat.")?;
            if actor.is_minion() {
                return Err("This team chat is read-only. Copy the comment to its parent Bot.");
            }
            manager.chat_for_action(bot, cx).ok_or(
                "Open the originating Bot chat and wait for its draft to load, then try again.",
            )
        })?;
        chat.update(cx, |chat, cx| chat.append_review_comment(text, cx))
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
        let origin = self.workspace.active;
        let standalone = origin.is_none_or(|id| self.workspace.session_for_pane(id).is_none());
        let id = if standalone {
            let bot = origin
                .and_then(|id| self.workspace.app_panes.get(&id))
                .and_then(|pane| match &pane.content {
                    AppPane::Bot(bot) | AppPane::BotDocument { bot, .. } => Some(bot.clone()),
                    AppPane::Review => pane.review_bot.clone(),
                    _ => None,
                });
            let Some(id) = self.workspace.standalone_review_pane(
                self.workspace.project.clone(),
                self.working_copy().to_owned(),
                bot,
            ) else {
                return;
            };
            id
        } else {
            // Reopen a retained file pane with its existing dirty editor.
            let retained = self.workspace.review_owners.keys().copied().find(|pane| {
                self.workspace.tab_of(*pane).is_none()
                    && self.pane_in_scope(*pane, cx)
                    && self
                        .workspace
                        .reviews
                        .get(pane)
                        .is_some_and(|r| r.tabs.iter().any(|existing| existing.same_tab(&tab)))
            });
            if let Some(pane) = retained {
                self.open_file_pane(pane, window, cx);
            }
            self.workspace.active.unwrap()
        };
        let review = match self.review_for(id, cx) {
            Some(review) => review,
            None if standalone => self.make_review(
                id,
                crate::git_tools::review::ReviewState {
                    chat_hidden: true,
                    ..Default::default()
                },
                cx,
            ),
            None => {
                let review = self.make_review(id, Default::default(), cx);
                let ratio = review.read(cx).state.ratio;
                let distance = self.split_width() * ratio;
                let slide = match self.review.slide.take() {
                    Some((sid, slide, _)) if sid == id => {
                        slide.retarget(ratio, vorkspace_ui::clock::Instant::now())
                    }
                    _ => PaneSlide::new(0., ratio, distance),
                };
                self.review.slide = Some((id, slide, review.clone()));
                review
            }
        };
        let entering = self
            .review
            .slide
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
        if standalone {
            // Publish the editor state with its layout even before Changed is delivered.
            self.workspace
                .reviews
                .insert(id, review.read(cx).state.clone());
            if self.workspace.tab_of(id).is_none()
                && let Some(bot) = origin.filter(|origin| {
                    self.workspace
                        .app_panes
                        .get(origin)
                        .is_some_and(|pane| matches!(pane.content, AppPane::Bot(_)))
                })
                && self.placement_allowed(DragSource::Pane(id), bot, Edge::Right)
            {
                self.workspace.place(id, bot, Edge::Right);
                self.fit_active_pane_width();
            }
            self.open(id, window, cx);
            return;
        }
        self.review.focused = true;
        review.update(cx, |r, cx| r.focus_active(window, cx));
        cx.notify();
    }

    pub(super) fn open_standalone_review(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(review) = self.review_for(id, cx) else {
            return;
        };
        self.prepare_app_view(Some(id), window, cx);
        self.leave_app_views(cx);
        self.workspace.open(id);
        self.history.record(id);
        self.review.focused = true;
        review.update(cx, |review, cx| review.set_visible(true, cx));
        self.remember_title_tab(TitleTab::Workspace(self.workspace.tab_of(id).unwrap()));
        self.finish_app_pane_open(window, cx);
    }

    /// The pane's last tab closed: it slides out, and a tab without its
    /// chat closes with it.
    pub(super) fn close_review(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(review) = self.review.reviews.remove(&id) else {
            return;
        };
        review.update(cx, |r, cx| r.cache_closed(cx));
        let chat_hidden = review.read(cx).state.chat_hidden;
        let ratio = review.read(cx).state.ratio;
        self.workspace.reviews.remove(&id);
        if self.workspace.active == Some(id) {
            self.review.focused = false;
            if !chat_hidden {
                // The closing pane stays mounted for its slide-out. Move focus
                // before it disappears, rather than leaving keys in its editor.
                self.pending_composer_focus = self.workspace.session_for_pane(id);
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
                    if this.review.reviews.contains_key(&id) {
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
            let slide = match self.review.slide.take() {
                Some((sid, slide, _)) if sid == id => {
                    slide.retarget(0., vorkspace_ui::clock::Instant::now())
                }
                _ => PaneSlide::new(ratio, 0., distance),
            };
            self.review.slide = Some((id, slide, review));
        }
        self.persist(cx);
        cx.notify();
    }
    pub(super) fn drag_review(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.review.drag {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.review.drag = false;
            self.persist(cx);
            return;
        }
        let bounds = self
            .workspace
            .active
            .and_then(|id| self.panes.split_bounds.get(&id))
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
