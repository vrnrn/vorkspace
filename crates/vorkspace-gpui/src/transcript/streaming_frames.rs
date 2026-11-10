//! Streaming state and the frame clock that drives live reveal.
use super::*;

#[derive(Default)]
pub(super) struct Reveals {
    pub(super) active: HashMap<usize, PacedText>,
    // Text received while hidden or with pacing disabled stays fully shown
    // if a later patch rebuilds the same message.
    settled_before: usize,
}

impl Reveals {
    pub(super) fn settle_through(&mut self, source_len: usize) {
        self.active = HashMap::new();
        self.settled_before = source_len;
    }
}

impl TranscriptView {
    pub(super) fn should_pace(&self, at: usize) -> bool {
        self.streaming
            && self.visible
            && self.smooth_reveal
            && at >= self.live_start.max(self.reveals.settled_before)
    }

    pub(super) fn prepare_reveal(&mut self, at: usize, end: usize, text: &str, now: Instant) {
        let start_pacing = self.should_pace(at);
        let paced = match self.reveals.active.entry(at) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) if start_pacing => {
                entry.insert(PacedText::new(text, now))
            }
            _ => return,
        };
        paced.update(text, now);
        paced.step(now, self.streaming && end == self.source.len());
    }

    pub(super) fn answer_visible(&self, answer: usize) -> bool {
        if self.source[answer].text.trim().is_empty() {
            return false;
        }
        if answer < self.live_start.max(self.reveals.settled_before)
            || !self.visible
            || !self.smooth_reveal
        {
            return true;
        }
        self.reveals
            .active
            .get(&answer)
            .map_or(!self.streaming, |r| !r.text().trim().is_empty())
    }

    /// Stop visible work immediately, while the provider drains and saves.
    pub(crate) fn stop_streaming(&mut self, cx: &mut Context<Self>) {
        let start = self
            .source
            .iter()
            .rposition(Row::is_prompt)
            .map_or(0, |at| at + 1);
        for row in self
            .source
            .iter_mut()
            .skip(start)
            .filter(|row| row.role == "tool")
        {
            if let Ok(mut tool) = serde_json::from_str::<ToolCall>(&row.text)
                && tool.state == ToolState::Running
            {
                tool.state = ToolState::Cancelled;
                if tool.output.is_empty() {
                    tool.output = "Cancelled".into();
                }
                let previous_bytes = row.text.len();
                row.text = serde_json::to_string(&tool).unwrap_or_else(|_| row.text.clone());
                self.source_bytes = self
                    .source_bytes
                    .saturating_sub(previous_bytes)
                    .saturating_add(row.text.len());
            }
        }
        self.reveals.settle_through(self.source.len());
        self.streaming(false, cx);
        self.rebuild(start, cx);
        cx.notify();
    }

    pub(super) fn sync_streaming_clock(&mut self, cx: &mut Context<Self>) {
        if !self.streaming || !self.visible {
            self.streaming_clock = None;
        } else if self.streaming_clock.is_none() {
            // A turn and its visible surface own one elapsed-time wakeup.
            // Dropping the task on stop/hide prevents a previous turn's
            // sleeping loop from joining a replacement turn.
            self.streaming_clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1))
                        .await;
                    if !this
                        .update(cx, |this, cx| {
                            let active = this.streaming && this.visible;
                            if active {
                                cx.notify();
                            }
                            active
                        })
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            }));
        }
    }

    pub(crate) fn streaming(&mut self, streaming: bool, cx: &mut Context<Self>) {
        if self.streaming == streaming {
            return;
        }
        self.streaming = streaming;
        self.sync_streaming_clock(cx);
        if streaming {
            self.active_provider_turn_id = None;
            self.live_start = self.source.len();
            self.reveals.settled_before = 0;
            // The provider starts before its prompt arrives. Keep an ordinary
            // chat's existing spacer and scroll position until patch accepts
            // that prompt, just as React pins on lastUserId rather than busy.
            // Otherwise the old spacer collapses for a frame before the new
            // prompt gets its own anchor and entrance.
            if self.bot {
                self.show_jump = false;
                self.list.set_follow_mode(FollowMode::Tail);
            }
            self.started = Some(vorkspace_ui::clock::Instant::now());
        } else {
            self.reveals
                .active
                .retain(|_, reveal| reveal.active(Instant::now()));
            self.follow.until = Some(Instant::now() + std::time::Duration::from_millis(1200));
            if let Some(started) = self.started.take()
                && let Some(turn) = self.active_turn()
            {
                self.elapsed.insert(turn, started.elapsed());
                let completed_at = vorkspace_core::time::now_ms();
                let started_at =
                    Some(completed_at.saturating_sub(started.elapsed().as_millis() as u64));
                let info = self.turns.entry(turn).or_default();
                info.provider = self.provider.clone();
                info.model = self.model.clone();
                info.started_at = started_at;
                info.duration_ms = started.elapsed().as_millis() as u64;
                info.duration_known = true;
                info.completed_at = started_at.and_then(|at| at.checked_add(info.duration_ms));
            }
        }
        // Starting a normal turn must not re-open the previous turn's work or
        // rebuild it as live while the new prompt is still in transit.
        if (!streaming || self.bot) && !self.messages.is_empty() {
            let start = self
                .source
                .iter()
                .rposition(|r| r.is_prompt())
                .map_or(0, |i| i + 1);
            self.rebuild(start, cx);
        } else {
            cx.notify();
        }
    }
    pub(super) fn schedule_frame(
        &mut self,
        resized: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = self.frame_now;
        if self.visible
            && (resized
                || !self.motions.is_empty()
                || self.agent_panels.animating()
                || self.prompt_rise.is_some_and(|(_, born)| {
                    now.saturating_duration_since(born).as_secs_f32() < 0.88
                })
                || self.reveals.active.values().any(|r| r.active(now))
                || self.chat_animating())
            && !self.frame_scheduled
        {
            self.frame_scheduled = true;
            let view = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = view.update(cx, |this, cx| this.animate_frame(cx));
            });
        }
    }
    pub(super) fn animate_frame(&mut self, cx: &mut Context<Self>) {
        self.frame_scheduled = false;
        // A frame queued before switching tabs must not invalidate the hidden
        // transcript's retained layout or advance its presentation animations.
        if !self.visible {
            return;
        }
        let mut changed_phases = HashSet::new();
        let panels = self.agent_panels.animate();
        let heights_changed = !panels.is_empty() || !self.motions.is_empty();
        for start in panels {
            for (ix, row) in self.rows.iter().enumerate() {
                if row.key.0 == start {
                    self.list.remeasure_items(ix..ix + 1);
                    changed_phases.extend(row.phase_group());
                }
            }
        }
        let now = self.animation_now();
        let mut changed = None;
        for (&at, reveal) in &mut self.reveals.active {
            if reveal.step(now, self.streaming) {
                changed = Some(changed.map_or(at, |old: usize| old.min(at)));
            }
        }
        if let Some(start) = changed {
            self.rebuild(start, cx);
        }
        self.reveals
            .active
            .retain(|&at, reveal| (self.streaming && at >= self.live_start) || reveal.active(now));
        let mut removed = Vec::new();
        if !self.motions.is_empty() {
            for (ix, row) in self.rows.iter().enumerate() {
                if let Some(motion) = self.motions.get(&row.key) {
                    changed_phases.extend(row.phase_group());
                    if motion.finished(now) && !motion.opening {
                        removed.push(ix);
                    } else {
                        self.list.remeasure_items(ix..ix + 1);
                    }
                }
            }
        }
        if !changed_phases.is_empty() {
            for (ix, row) in self.rows.iter().enumerate() {
                if matches!(row.body, Body::Phase { group, .. } if changed_phases.contains(&group) && self.windowed_phase(group))
                {
                    self.list.remeasure_items(ix..ix + 1);
                }
            }
        }
        if let Some(&start) = removed.first() {
            let offset = self.list.logical_scroll_top();
            let anchor = (!self.list.is_following_tail())
                .then(|| self.rows.get(offset.item_ix).map(|r| r.key))
                .flatten();
            self.invalidate_message_indices(start);
            for ix in removed.into_iter().rev() {
                self.rows.remove(ix);
                self.list.splice(ix..ix + 1, 0);
            }
            if let Some(key) = anchor
                && let Some(item_ix) = self.rows.iter().position(|r| r.key == key)
                && item_ix != offset.item_ix
            {
                self.list.scroll_to(ListOffset {
                    item_ix,
                    offset_in_item: offset.offset_in_item,
                });
            }
            self.index_messages_from(start);
        }
        self.motions.retain(|_, motion| !motion.finished(now));
        self.entrances
            .retain(|_, born| now.duration_since(*born).as_secs_f32() < 0.4);
        self.pending_entrances
            .retain(|_, born| now.duration_since(*born).as_secs_f32() < 0.4);
        for (_, born) in self.reactions.values_mut() {
            if born.is_some_and(|b| now.duration_since(b).as_secs_f32() >= 0.4) {
                *born = None;
            }
        }
        if heights_changed {
            self.list
                .remeasure_items(self.footer_ix()..self.footer_ix() + 1);
        }
        cx.notify();
    }
}
