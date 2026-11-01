//! Following the live tail: what stays anchored as content grows.
use super::*;

impl TranscriptView {
    /// Measures the live turn's tail before the list lays it out, so growth
    /// the list follows is painted as an offset that eases away.
    pub(super) fn update_follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = self.frame_now;
        let animated = self.streaming || self.follow.until.is_some_and(|until| now < until);
        if self.document || !self.visible {
            self.follow = Follow::default();
            return;
        }
        if !self.list.is_following_tail() {
            // Hand the painted position to native scrolling once, like React's pause.
            self.list.scroll_by(px(-self.follow.offset));
            self.follow = Follow::default();
            return;
        }
        if self.viewport.size.width <= px(0.) {
            return;
        }
        let turn = self
            .source
            .iter()
            .rposition(|r| r.is_prompt())
            .map_or(0, |ix| ix + 1);
        let prompt = self.prompt_start(self.bot_group_start(turn));
        let group = self
            .messages
            .iter()
            .find(|m| m.start >= prompt)
            .map_or(0, |m| m.display_start);
        let tail = self.rows.len().saturating_sub(FOLLOW_ROWS);
        let baseline = group.max(tail);
        // Include the previous anchor for the handover to a new turn. Store only
        // the new group's baseline afterwards, so old turns aren't measured forever.
        let mut from = self
            .follow
            .measured
            .as_ref()
            .and_then(|m| m.rows.first())
            .and_then(|(key, _)| self.rows[..baseline].iter().position(|r| r.key == *key))
            .unwrap_or(baseline);
        let mut measured = FollowMeasure {
            viewport: self.viewport.size,
            bottom: self.viewport.bottom(),
            // These bounds belong to the previous layout until prepaint gives
            // us the current viewport, including the composer's new height.
            window_size: self
                .follow
                .measured
                .as_ref()
                .map_or_else(|| window.viewport_size(), |m| m.window_size),
            rows: Vec::new(),
            footer: 0.,
            slack: 0.,
        };
        for ix in from..self.rows.len() {
            let height = f32::from(self.measure_row_height(ix, window, cx));
            measured.rows.push((self.rows[ix].key, height));
        }
        if !self.bot {
            measured.footer = f32::from(self.footer_height(window, cx));
            let mut height = TOP_INSET
                + measured.footer
                + measured.rows.iter().map(|(_, height)| height).sum::<f32>();
            // A short tail can still belong to a long conversation. Measure
            // preceding rows only until the viewport is filled, so underflow
            // is exact without measuring the whole history on each frame.
            let mut leading = Vec::new();
            while from > 0 && height < f32::from(self.viewport.size.height) {
                from -= 1;
                let row_height = f32::from(self.measure_row_height(from, window, cx));
                height += row_height;
                leading.push((self.rows[from].key, row_height));
            }
            leading.reverse();
            leading.append(&mut measured.rows);
            measured.rows = leading;
            measured.slack = (f32::from(self.viewport.size.height) - height).max(0.);
        }
        // The spacer pins the prompt in native layout. A follow spring left
        // over from taller content must never paint it below that position.
        let prompt_overflow = self.anchor_row().and_then(|first| {
            if measured.footer > BOTTOM_INSET {
                Some(0.)
            } else if first >= from {
                Some(
                    (TOP_INSET
                        + BOTTOM_INSET
                        + measured.rows[first - from..]
                            .iter()
                            .map(|(_, height)| height)
                            .sum::<f32>()
                        - f32::from(self.viewport.size.height))
                    .max(0.),
                )
            } else {
                None
            }
        });
        let resized = self.follow.measured.as_ref().is_some_and(|m| {
            measured.resized_from(m) || (!self.bot && measured.viewport.height != m.viewport.height)
        });
        let shift = self.follow.measured.as_ref().map_or(0., |previous| {
            measured.shift_from(previous) - self.closing_shrink(&measured, previous)
        });
        measured.rows.drain(..baseline - from);
        self.follow.measured = Some(measured);
        if resized || prompt_overflow == Some(0.) || (!animated && shift.abs() > 0.) {
            self.follow.reset();
            return;
        }
        let mut elapsed = self
            .follow
            .last_frame
            .map_or(0., |at| now.duration_since(at).as_secs_f32() * 1000.)
            .min(64.);
        #[cfg(test)]
        if let Some(frame_elapsed) = self.follow.frame_elapsed {
            elapsed = frame_elapsed;
        }
        self.follow.last_frame = Some(now);
        if animated && shift.abs() > 0. {
            if self.follow.offset.abs() < 0.25 {
                elapsed = 0.;
            }
            if self.follow.offset.abs() < 0.25
                && self
                    .follow
                    .settled_at
                    .is_none_or(|at| now.duration_since(at).as_secs_f32() > 0.5)
            {
                self.follow.velocity = 0.;
                self.follow.growth = 0.;
                self.follow.last_step = Some(now);
            }
            self.follow.settled_at = None;
            self.follow
                .add_shift(shift, f32::from(self.viewport.size.height), self.bot);
        }
        if let Some(overflow) = prompt_overflow {
            self.follow.offset = self.follow.offset.min(overflow);
        }
        // As in React's measure/tick split, the first growth frame only
        // records distance. Sample its velocity after real frame time passes.
        if elapsed == 0. {
            return;
        }
        if self.follow.offset > 0. {
            let sample = self
                .follow
                .last_step
                .map_or(0., |at| now.duration_since(at).as_secs_f32() * 1000.)
                / SPRING_FRAME;
            let grown = self.follow.grown;
            self.follow.offset = self.follow.step(
                self.follow.offset,
                grown,
                elapsed / SPRING_FRAME,
                sample.min(60.),
                if self.bot {
                    CHAT_FOLLOW_MAX_SPEED
                } else {
                    f32::INFINITY
                },
            );
        } else {
            self.follow.offset *= (-elapsed / 85.).exp();
        }
        self.follow.grown = 0.;
        self.follow.last_step = Some(now);
        if self.follow.offset.abs() < 0.25 && self.follow.offset != 0. {
            self.follow.settle(now);
        }
    }
    /// Content is measured before layout; compensate for vertical layout once
    /// its actual bounds are known, before the list builds its painted rows.
    pub(super) fn update_follow_viewport(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &Window,
    ) -> bool {
        let Some(measured) = self.follow.measured.as_mut() else {
            return false;
        };
        let resized = measured.viewport.width != bounds.size.width
            || measured.window_size != window.viewport_size()
            || (!self.bot && measured.viewport.height != bounds.size.height);
        let shift = f32::from(measured.bottom - bounds.bottom());
        measured.viewport = bounds.size;
        measured.bottom = bounds.bottom();
        measured.window_size = window.viewport_size();
        if resized {
            self.follow.reset();
            // The old rows may wrap differently at the new width.
            self.follow.measured = None;
        } else if shift != 0. {
            let now = Instant::now();
            if self.streaming
                || self.follow.until.is_some_and(|until| now < until)
                || self.follow.offset.abs() >= 0.25
            {
                self.follow
                    .add_shift(shift, f32::from(bounds.size.height), self.bot);
            } else {
                self.follow.reset();
            }
        }
        resized
    }
    /// Height lost by rows easing closed, such as work folding behind its
    /// line. Their motion already eases it; following must not ease it again.
    pub(super) fn closing_shrink(&self, measured: &FollowMeasure, previous: &FollowMeasure) -> f32 {
        if measured.resized_from(previous) {
            return 0.;
        }
        let before: HashMap<_, _> = previous.rows.iter().copied().collect();
        let shrink: f32 = measured
            .rows
            .iter()
            .filter(|(key, _)| self.motions.get(key).is_some_and(|m| !m.opening))
            .filter_map(|(key, height)| before.get(key).map(|old| height - old))
            .sum();
        // An anchored prompt's spacer absorbs closing height, so nothing
        // above moves. Only height beyond the spare space shifts the chat.
        let spare = ((previous.footer - BOTTOM_INSET).max(0.) + previous.slack).max(0.);
        if shrink < 0. {
            (shrink + spare).min(0.)
        } else {
            shrink
        }
    }
    pub(super) fn update_jump(&mut self, cx: &mut Context<Self>) {
        let show =
            !self.list.is_following_tail() && self.list.max_offset_for_scrollbar().y > px(1.);
        if self.show_jump != show {
            self.show_jump = show;
            cx.notify();
        }
    }
    pub(super) fn jump_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.follow = Follow::default();
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        self.show_jump = false;
        cx.notify();
    }
}
