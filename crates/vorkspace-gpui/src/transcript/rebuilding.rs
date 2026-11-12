//! Turning transcript rows into presentation rows, incrementally where possible.
use super::*;

struct Rebuilding {
    now: Instant,
    following: bool,
    offset: ListOffset,
    anchor: Option<(usize, usize)>,
    current_turn: usize,
    layout_changed: bool,
    source_start: usize,
    display_start: usize,
    old_count: usize,
    streaming_prose: bool,
    old_rows: Vec<UiRow>,
    previous: HashMap<usize, Message>,
    activity: phases::Phases,
    cards: cards::Cards,
    turn: usize,
    turn_end: usize,
    final_answer: Option<usize>,
    awaiting_answer: Option<usize>,
    has_work: bool,
    ready: bool,
    receipt_start: Option<usize>,
    last_prompt: Option<usize>,
}

struct RebuildMessage<'a> {
    at: usize,
    end: usize,
    role: String,
    is_prompt: bool,
    text: String,
    peer_origin: Option<vorkspace_core::bot::peers::Origin>,
    stopped_notice: bool,
    display_text: Option<String>,
    attachments: Arc<[attachments::Attachment]>,
    parser: MarkdownParser,
    message: usize,
    first: usize,
    phase: Option<&'a phases::Group>,
    grouped: bool,
    phase_open: bool,
    folded: bool,
    reaction: bool,
}

impl TranscriptView {
    pub(crate) fn patch(&mut self, start: usize, rows: Vec<Row>, cx: &mut Context<Self>) {
        self.patch_with_page(start, rows, None, cx);
    }

    pub(super) fn patch_with_page(
        &mut self,
        start: usize,
        rows: Vec<Row>,
        initial_page: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if rows.is_empty() {
            return;
        }
        assert!(
            start <= self.source.len(),
            "transcript patches must be contiguous"
        );
        let known = self.source.len();
        for (ix, row) in rows.into_iter().enumerate() {
            let ix = start + ix;
            let bytes = row.text.capacity().saturating_add(row.role.capacity());
            if ix < self.source.len() {
                if !row.first || row.role != "assistant" {
                    self.reveals.active.remove(&ix);
                }
                let old = &self.source[ix];
                if self.bot
                    && self.streaming
                    && self.visible
                    && ix >= self.live_start
                    && row.first
                    && row.role == "assistant"
                    && old.text.trim().is_empty()
                    && !row.text.trim().is_empty()
                {
                    // An existing empty provider placeholder gains its first
                    // words now. Completion must not replay this entrance.
                    self.entrances.insert(ix, Instant::now());
                }
                self.source_bytes = self
                    .source_bytes
                    .saturating_sub(old.text.capacity().saturating_add(old.role.capacity()));
                self.source[ix] = row;
            } else {
                self.source.push(row);
            }
            self.source_bytes = self.source_bytes.saturating_add(bytes);
        }
        if !self.visible || !self.smooth_reveal {
            self.reveals.settle_through(self.source.len());
        }
        self.visuals.retain(|&start, _| {
            self.source
                .get(start)
                .is_some_and(|row| row.role == "visual")
        });
        if known == 0 && self.bot && !self.streaming {
            self.history_start = initial_page
                .unwrap_or_else(|| history::page_start(&self.source, self.source.len()));
        }
        if self.bot {
            let affected = self.source[..start.min(self.source.len())]
                .iter()
                .rposition(|row| row.first && row.is_prompt())
                .unwrap_or(0);
            self.bot_groups.get_mut().retain(|&turn, _| turn < affected);
        }
        if self.streaming
            && let Some(id) = self.active_provider_turn_id.clone()
            && let Some(turn) = self.active_turn()
        {
            self.turns.entry(turn).or_default().provider_turn_id = Some(id);
        }
        if self.streaming
            && let Some(turn) = self.active_turn()
        {
            let provider = self.provider.clone();
            let model = self.model.clone();
            self.turns
                .entry(turn)
                .or_insert_with(|| vorkspace_core::agent::TurnInfo {
                    provider,
                    model,
                    ..Default::default()
                });
        }
        if self.streaming {
            for prompt in known.max(self.live_start)..self.source.len() {
                if self.source[prompt].first && self.source[prompt].is_prompt() {
                    let turn = (prompt + 1..self.source.len())
                        .find(|&at| self.source[at].first)
                        .unwrap_or(self.source.len());
                    let provider = self.provider.clone();
                    let model = self.model.clone();
                    let provider_turn_id = self.active_provider_turn_id.clone();
                    self.turns
                        .entry(turn)
                        .or_insert_with(|| vorkspace_core::agent::TurnInfo {
                            provider,
                            model,
                            provider_turn_id,
                            ..Default::default()
                        });
                }
            }
        }
        // React pins on a change to lastUserId, including an accepted steer
        // inside the current provider turn. Continuation rows and replayed
        // snapshots must not move the anchor or resume following the tail.
        if self.streaming
            && !self.bot
            && let Some(prompt) = (known..self.source.len())
                .rfind(|&ix| self.source[ix].first && self.source[ix].is_prompt())
        {
            self.anchor_turn = self.anchor_prompts.then_some(prompt);
            // React's ordinary transcript introduces a sent prompt itself.
            // Start a fresh follow baseline instead of adding a second glide.
            self.follow = Follow::default();
            self.show_jump = false;
            self.list.set_follow_mode(FollowMode::Tail);
        }
        if let Some(prompt) = self.anchor_turn
            && self.source.get(prompt).is_some_and(|r| r.is_prompt())
            && self.introduced_prompt != Some(prompt)
            && self.streaming
            && start <= prompt
        {
            self.introduced_prompt = Some(prompt);
            if self.visible {
                self.prompt_rise = Some((prompt, Instant::now()));
                self.prompt_rise_pending = true;
            }
        }
        // Messages that arrive while the chat is open slide and fade in;
        // history that loads or replays does not.
        if self.bot && self.streaming && self.visible {
            let now = Instant::now();
            for ix in start.max(known).max(self.live_start)..self.source.len() {
                let row = &self.source[ix];
                if !row.first || !matches!(row.role.as_str(), "user" | "assistant") {
                    continue;
                }
                let queued = if row.role == "user" {
                    // A long prompt spans storage rows. Match its full text so
                    // wrapping and newline chunking don't create another entrance.
                    let end = (ix + 1..self.source.len())
                        .find(|&at| self.source[at].first)
                        .unwrap_or(self.source.len());
                    let text: String = self.source[ix..end]
                        .iter()
                        .map(|r| r.text.as_str())
                        .collect();
                    self.pending
                        .iter()
                        .find(|message| text.starts_with(message.text.as_ref()))
                        .map(|message| self.pending_entrances.get(&message.id).copied())
                        .or_else(|| {
                            self.taken
                                .iter()
                                .rev()
                                .find(|message| {
                                    now.duration_since(message.removed_at).as_secs_f32() < 2.
                                        && text.starts_with(message.text.as_ref())
                                })
                                .map(|message| message.entrance)
                        })
                } else {
                    None
                };
                // Delivery keeps the same animation clock as the optimistic
                // bubble. A settled bubble stays settled; an in-flight slide
                // continues even when clearing a multiline composer resizes us.
                let entrance = queued.map_or(Some(now), |born| {
                    born.filter(|born| now.duration_since(*born).as_secs_f32() < 0.36)
                });
                if let Some(born) = entrance {
                    self.entrances.insert(ix, born);
                }
            }
        }
        self.outline.patched(&self.source, start);
        self.rebuild(start, cx);
    }
    pub(super) fn rebuild(&mut self, start: usize, cx: &mut Context<Self>) {
        let mut building = self.begin_rebuild(start);
        self.rebuild_messages(&mut building);
        self.finish_rebuild(building, cx);
    }

    fn rebuild_target(&mut self, start: usize, now: Instant) -> (usize, usize, bool) {
        let current_turn = self.source[..(start + 1).min(self.source.len())]
            .iter()
            .rposition(|r| r.is_prompt())
            .map_or(0, |i| i + 1);
        let current_end = (current_turn..self.source.len())
            .find(|&i| self.source[i].is_prompt())
            .unwrap_or(self.source.len());
        // Advance incoming prose before choosing the displayed block. Its
        // first visible words and the previous block's replacement must be
        // committed together, including after an empty provider placeholder.
        if let Some(at) = (current_turn..current_end)
            .rev()
            .find(|&i| self.source[i].role == "assistant" && self.source[i].first)
            && (self.should_pace(at) || self.reveals.active.contains_key(&at))
        {
            let end = (at + 1..current_end)
                .find(|&i| self.source[i].first)
                .unwrap_or(current_end);
            let text: String = self.source[at..end]
                .iter()
                .map(|r| r.text.as_str())
                .collect();
            self.prepare_reveal(at, end, &text, now);
        }
        let candidate = (current_turn..current_end).rev().find(|&i| {
            self.source[i].role == "assistant" && self.source[i].first && self.answer_visible(i)
        });
        let target = candidate.filter(|&answer| {
            self.source[current_turn..answer]
                .iter()
                .any(|r| r.role == "tool")
        });
        let layout_changed = self.fold_targets.get(&current_turn).copied().flatten() != target;

        (current_turn, current_end, layout_changed)
    }

    fn rebuild_message_index(
        &self,
        start: usize,
        current_turn: usize,
        current_end: usize,
        layout_changed: bool,
    ) -> usize {
        let known_message = self
            .source
            .get(start)
            .is_some_and(|r| r.role == "assistant")
            && self
                .messages
                .iter()
                .take_while(|m| m.start <= start)
                .any(|m| m.role == "assistant");
        let mut message_ix = self
            .messages
            .partition_point(|m| m.start <= start)
            .saturating_sub(1);
        while message_ix > 0
            && matches!(
                self.messages[message_ix].role.as_str(),
                "tool" | "reasoning"
            )
            && matches!(
                self.messages[message_ix - 1].role.as_str(),
                "tool" | "reasoning"
            )
        {
            message_ix -= 1;
        }
        if (layout_changed
            || !known_message
            || self.source.get(start).is_none_or(|r| r.role != "assistant"))
            && self.source[..start.min(self.source.len())]
                .iter()
                .rev()
                .take_while(|r| !r.is_prompt())
                .any(|r| r.role == "tool")
        {
            while message_ix > 0
                && !matches!(
                    self.messages[message_ix - 1].role.as_str(),
                    "user" | "bot-system"
                )
            {
                message_ix -= 1;
            }
        }
        message_ix = self.rebuild_pinned_start(message_ix, current_turn, current_end);
        if self.bot
            || self.source[current_turn..current_end]
                .iter()
                .any(|r| r.role == "bot-card")
        {
            // Resource reconciliation needs the whole affected response; Bot
            // completion runs also share identities with their opening run.
            // A completion run takes over the actions of the turn it
            // continues, so rebuild from the prompt that opens the group.
            let group = if self.bot {
                self.bot_group_start(current_turn)
            } else {
                current_turn
            };
            let prompt = self.prompt_start(group);
            while message_ix > 0
                && self
                    .messages
                    .get(message_ix)
                    .is_some_and(|m| m.start > prompt)
            {
                message_ix -= 1;
            }
        }

        message_ix
    }

    fn rebuild_pinned_start(
        &self,
        mut message_ix: usize,
        current_turn: usize,
        current_end: usize,
    ) -> usize {
        let run_message = self.messages.partition_point(|m| m.start < current_turn);
        let old_run_start = self.messages[run_message..]
            .iter()
            .filter_map(|m| m.display_start)
            .min()
            .unwrap_or(self.rows.len());
        if self.source[current_turn..current_end].iter().any(|row| {
                    row.role == "tool"
                        && row.first
                        && serde_json::from_str::<ToolCall>(&row.text)
                            .is_ok_and(|call| matches!(call.panel().as_deref(), Some(Panel::Agent(_))))
                }) || self.rows[old_run_start..].iter().any(|row| {
                    self.messages
                        .get(row.message)
                        .is_some_and(|message| message.run == current_turn)
                        && matches!(
                            &row.body,
                            Body::Tool { call, .. } if matches!(call.panel().as_deref(), Some(Panel::Agent(_)))
                        )
                }) {
                    // Pinned runs sit below the fold's work rather than at their
                    // original source offset. Rebuild the whole turn so a suffix
                    // update cannot cut across that reordered stack. Include the old
                    // rows: a patch can remove the run that caused the reordering.
                    while message_ix > 0 && self.messages[message_ix - 1].start >= current_turn {
                        message_ix -= 1;
                    }
                }

        message_ix
    }

    fn begin_rebuild(&mut self, start: usize) -> Rebuilding {
        #[cfg(test)]
        {
            self.rebuild_passes += 1;
        }
        self.phase_layouts.clear();
        let now = Instant::now();
        let following = self.list.is_following_tail();
        let offset = self.list.logical_scroll_top();
        let anchor = self.rows.get(offset.item_ix).map(|r| r.key);

        let (current_turn, current_end, layout_changed) = self.rebuild_target(start, now);
        let message_ix =
            self.rebuild_message_index(start, current_turn, current_end, layout_changed);
        let source_start = if message_ix == 0 {
            self.history_start
        } else {
            self.messages[message_ix].start
        };
        let display_start = if message_ix == 0 {
            0
        } else {
            // Pinned work can move out of source order. Cut before the
            // earliest displayed row owned by the rebuilt suffix.
            self.messages[message_ix..]
                .iter()
                .filter_map(|message| message.display_start)
                .min()
                .unwrap_or(self.rows.len())
        };
        let old_count = self.rows.len();
        let streaming_prose = self.streaming
            && !self.bot
            && !layout_changed
            && self.messages.last().is_some_and(|last| {
                last.start == source_start
                    && last.role == "assistant"
                    && self
                        .source
                        .get(source_start)
                        .is_some_and(|row| row.role == "assistant")
            })
            && current_end == self.source.len();
        self.invalidate_message_indices(display_start);
        let old_rows = self.rows.split_off(display_start);
        let previous_cards = cards::Previous::new(&old_rows, &self.messages);
        let previous: HashMap<_, _> = self
            .messages
            .split_off(message_ix)
            .into_iter()
            .map(|m| (m.start, m))
            .collect();
        if display_start == 0 && (self.history_start > 0 || self.storage_has_older()) {
            self.rows.push(UiRow {
                key: (self.history_start, 8),
                body: Body::OlderHistory,
                message: 0,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            });
        }
        let activity = phases::collect(&self.source, source_start, self.streaming);
        let cards = cards::Cards::collect(self, source_start, previous_cards);
        let turn = 0;
        let turn_end = 0;
        let final_answer = None;
        let awaiting_answer = None;
        let has_work = false;
        let ready = false;
        let receipt_start = None;
        let last_prompt = (self.bot && self.streaming)
            .then(|| self.source.iter().rposition(Row::is_prompt))
            .flatten();

        Rebuilding {
            now,
            following,
            offset,
            anchor,
            current_turn,
            layout_changed,
            source_start,
            display_start,
            old_count,
            streaming_prose,
            old_rows,
            previous,
            activity,
            cards,
            turn,
            turn_end,
            final_answer,
            awaiting_answer,
            has_work,
            ready,
            receipt_start,
            last_prompt,
        }
    }

    fn rebuild_messages(&mut self, building: &mut Rebuilding) {
        let mut at = building.source_start;
        while at < self.source.len() {
            at = self.rebuild_message(at, building);
        }
    }

    fn rebuild_message_turn(&mut self, at: usize, is_prompt: bool, building: &mut Rebuilding) {
        if at >= building.turn_end {
            building.turn = if is_prompt {
                (at..self.source.len())
                    .find(|&i| !self.source[i].is_prompt())
                    .unwrap_or(self.source.len())
            } else {
                self.source[..at]
                    .iter()
                    .rposition(|r| r.is_prompt())
                    .map_or(0, |i| i + 1)
            };
            building.turn_end = (building.turn..self.source.len())
                .find(|&i| self.source[i].is_prompt())
                .unwrap_or(self.source.len());
            building.awaiting_answer = (building.turn..building.turn_end)
                .rev()
                .find(|&i| self.source[i].role == "assistant" && self.source[i].first)
                .filter(|&i| !self.answer_visible(i));
            building.final_answer = (building.turn..building.turn_end).rev().find(|&i| {
                self.source[i].role == "assistant" && self.source[i].first && self.answer_visible(i)
            });
            building.has_work = building.final_answer.is_some_and(|answer| {
                self.source[building.turn..answer]
                    .iter()
                    .any(|r| r.role == "tool")
            });
            building.ready = building
                .final_answer
                .is_some_and(|answer| self.answer_visible(answer));
            self.fold_targets.insert(
                building.turn,
                building
                    .final_answer
                    .filter(|_| building.has_work && building.ready),
            );
            building.receipt_start = (building.turn..building.turn_end).find(|&i| {
                let row = &self.source[i];
                row.first
                    && !matches!(
                        row.role.as_str(),
                        "bot-card"
                            | "visual"
                            | "bot-document"
                            | "bot-note"
                            | "bot-launch"
                            | vorkspace_core::transcript::attachments::ROLE
                    )
                    && !(row.role == "error" && row.text.trim() == "Stopped")
                    && !(self.bot && row.text.trim().starts_with("<reaction>"))
            });
        }
    }

    fn rebuild_work_receipt(&mut self, at: usize, message: usize, building: &Rebuilding) {
        if !self.document && building.receipt_start == Some(at) {
            if self.bot
                && (self.bot_group_start(building.turn) == building.turn
                    || (self.history_start > 0
                        && self.prompt_start(building.turn) == self.history_start))
            {
                self.rows.push(UiRow {
                    key: (0, 0),
                    body: Body::BotHeader {
                        turn: self.bot_group_start(building.turn),
                    },
                    message,
                    first: false,
                    last: false,
                    work: false,
                    work_tail: false,
                    thought_phase: None,
                });
            }
            self.rows.push(UiRow {
                key: (0, 0),
                body: Body::WorkLine {
                    turn: building.turn,
                    expandable: building.has_work && building.ready,
                },
                message,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            });
        }
    }

    fn rebuild_message(&mut self, at: usize, building: &mut Rebuilding) -> usize {
        let mut end = at + 1;
        while end < self.source.len() && !self.source[end].first {
            end += 1;
        }
        let role = self.source[at].role.clone();
        let is_prompt = self.source[at].is_prompt();
        let text: String = self.source[at..end]
            .iter()
            .map(|r| r.text.as_str())
            .collect();
        let peer = (role == "bot-system")
            .then(|| vorkspace_core::bot::peers::presentation(&text))
            .flatten();
        let peer_origin = peer.as_ref().map(|(origin, _)| origin.clone());
        // Older native versions saved user cancellation as an error row.
        let stopped_notice = role == "error" && text.trim() == "Stopped";
        let (display_text, attachments) = if let Some((_, body)) = peer {
            (Some(body.to_owned()), Arc::default())
        } else if role == "user" {
            attachments::message(&self.source, end, &text)
        } else {
            (None, Arc::default())
        };
        let parser = building
            .previous
            .remove(&at)
            .filter(|p| p.role == role)
            .map(|p| p.parser)
            .or_else(|| self.prepared_parsers.remove(&at))
            .unwrap_or_default();
        let message = self.messages.len();
        let first = self.rows.len();

        self.rebuild_message_turn(at, is_prompt, building);
        let folded = !is_prompt
            && !building.activity.pinned.contains(&at)
            && building.has_work
            && building.ready
            && building.final_answer.is_some_and(|answer| at < answer)
            && !self
                .work_folds
                .get(&building.turn)
                .copied()
                .unwrap_or(false);

        self.rebuild_work_receipt(at, message, building);
        let phase = building.activity.at(at);
        let grouped = phase.is_some_and(|group| group.steps > 1);
        let active = phase.is_some_and(|group| {
            self.streaming
                && (group.end == self.source.len() || building.awaiting_answer == Some(group.end))
        });
        let phase_open = phase.is_none_or(|group| {
            !grouped || self.phases.get(&group.start).copied().unwrap_or(active)
        });

        if !folded && grouped && phase.is_some_and(|group| group.start == at) {
            let group = phase.unwrap();
            let (title, icon) = if group.calls.is_empty() {
                (if active { "Thinking…" } else { "Thought" }.into(), "minus")
            } else {
                crate::activity::summary(&group.calls, active)
            };
            self.rows.push(UiRow {
                key: (0, 0),
                body: Body::Phase {
                    title,
                    group: group.start,
                    icon,
                    active,
                },
                message,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            });
        }

        let reaction = self.bot && role == "assistant" && text.trim().starts_with("<reaction>");

        let mut message = RebuildMessage {
            at,
            end,
            role,
            is_prompt,
            text,
            peer_origin,
            stopped_notice,
            display_text,
            attachments,
            parser,
            message,
            first,
            phase,
            grouped,
            phase_open,
            folded,
            reaction,
        };
        self.rebuild_message_body(&mut message, building);
        self.finish_rebuild_message(message, building)
    }

    fn rebuild_interjection(&mut self, draft: &RebuildMessage<'_>) {
        if let Ok(note) = serde_json::from_str(&draft.text) {
            self.rows.push(UiRow {
                key: (0, 0),
                body: Body::Interjection(note),
                message: draft.message,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            });
        }
    }

    fn rebuild_visual(&mut self, draft: &RebuildMessage<'_>) {
        let document = serde_json::from_str::<vorkspace_core::visual::VisualDocument>(&draft.text)
            .ok()
            .filter(|document| document.validate().is_ok());
        let body = match document {
            Some(document) => Body::Visual(Arc::new(document)),
            None => Body::Plain("This visual could not be loaded.".into()),
        };
        self.rows.push(UiRow {
            key: (0, 0),
            body,
            message: draft.message,
            first: false,
            last: false,
            work: false,
            work_tail: false,
            thought_phase: None,
        });
    }

    fn rebuild_bot_card(&mut self, draft: &RebuildMessage<'_>, building: &Rebuilding) {
        if !building.cards.owns(draft.at)
            && let Ok(value) = serde_json::from_str(&draft.text)
        {
            self.rows.push(UiRow {
                key: (0, 0),
                body: Body::BotCard(value),
                message: draft.message,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            });
        }
    }

    fn rebuild_reaction(&mut self, draft: &RebuildMessage<'_>, building: &Rebuilding) {
        // BotReactionBadge: the emoji sits on the message it answers.
        if let Some(emoji) = draft
            .text
            .trim()
            .strip_prefix("<reaction>")
            .and_then(|s| s.strip_suffix("</reaction>"))
            .filter(|e| e.chars().count() <= 16)
            && let Some(prompt) = self.source[..draft.at].iter().rposition(|r| r.is_prompt())
        {
            let prompt = self.source[..=prompt]
                .iter()
                .rposition(|r| r.first)
                .unwrap_or(prompt);
            let live = self.streaming && draft.at >= self.live_start && self.visible;
            match self.reactions.get(&prompt) {
                Some((current, _)) if current == emoji => {}
                _ => {
                    self.reactions
                        .insert(prompt, (emoji.into(), live.then_some(building.now)));
                }
            }
        }
    }

    fn rebuild_markdown(&mut self, draft: &mut RebuildMessage<'_>, building: &Rebuilding) {
        if draft.role == "reasoning" {
            self.rows.push(UiRow {
                key: (0, 0),
                body: Body::Reasoning,
                message: draft.message,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            });
        }
        if draft.role == "assistant" {
            self.prepare_reveal(draft.at, draft.end, &draft.text, building.now);
        }
        let paced = self
            .reveals
            .active
            .get(&draft.at)
            .filter(|_| draft.role == "assistant");
        draft
            .parser
            .set_text(paced.map_or(draft.text.as_str(), PacedText::text));
        let tree = if paced.is_some_and(PacedText::behind)
            || (self.streaming && draft.end == self.source.len())
        {
            draft.parser.display_tree()
        } else {
            draft.parser.tree().clone()
        };
        let mut word_cursor = 0;
        for block in tree
            .blocks
            .iter()
            .filter(|_| draft.role != "reasoning" || self.expanded.contains(&draft.at))
        {
            self.rows
                .extend(markdown::rows(&block.block).into_iter().map(|mut row| {
                    if paced.is_some() {
                        row.word_start = word_cursor;
                        word_cursor += markdown::word_count(&row);
                    }
                    UiRow {
                        key: (0, 0),
                        body: Body::Markdown(row),
                        message: draft.message,
                        first: false,
                        last: false,
                        work: false,
                        work_tail: false,
                        thought_phase: None,
                    }
                }));
        }
        if let Some(paced) = self
            .reveals
            .active
            .get_mut(&draft.at)
            .filter(|_| draft.role == "assistant")
        {
            paced.words(word_cursor, building.now);
        }
    }

    fn rebuild_plain(&mut self, draft: &RebuildMessage<'_>) {
        let text_rows = if let Some(text) = &draft.display_text {
            let mut transcript = vorkspace_core::transcript::Transcript::default();
            transcript.begin("user");
            transcript.append(text);
            transcript
                .rows
                .into_iter()
                .map(|row| row.text)
                .collect::<Vec<_>>()
        } else {
            self.source[draft.at..draft.end]
                .iter()
                .filter(|r| r.role != "bot-system")
                .map(|row| row.text.clone())
                .collect()
        };
        // `line-clamp-4`: a collapsed message shows its first lines only.
        let clamped = draft.role == "user"
            && !self.expanded_prompts.contains(&draft.at)
            && text_rows.len() > 1;
        if clamped {
            self.clamped_prompts.insert(draft.at);
        } else {
            self.clamped_prompts.remove(&draft.at);
        }
        let shown = if clamped { 1 } else { text_rows.len() };
        for text in text_rows.into_iter().take(shown) {
            self.rows.push(UiRow {
                key: (0, 0),
                body: Body::Plain(text.into()),
                message: draft.message,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            });
        }
    }

    fn rebuild_message_body(&mut self, draft: &mut RebuildMessage<'_>, building: &Rebuilding) {
        if draft.role == "interjection" {
            self.rebuild_interjection(draft);
        } else if draft.role == "visual" {
            self.rebuild_visual(draft);
        } else if matches!(draft.role.as_str(), "bot-card" | "bot-note") {
            self.rebuild_bot_card(draft, building);
        } else if draft.stopped_notice
            || matches!(
                draft.role.as_str(),
                "bot-document" | "bot-launch" | vorkspace_core::transcript::attachments::ROLE
            )
        {
            // Saved work references are rendered once under the settled reply.
        } else if draft.reaction {
            self.rebuild_reaction(draft, building);
        } else if draft.folded || building.activity.hidden.contains(&draft.at) || !draft.phase_open
        {
            if draft.role == "tool" {
                self.seen_steps.insert(draft.at);
            }
            // WorkFoldLine puts earlier commentary and work behind its receipt.
        } else if draft.role == "tool"
            && let Ok(tool) = serde_json::from_str::<ToolCall>(&draft.text)
        {
            self.rows.push(UiRow {
                key: (0, 0),
                body: Body::Tool {
                    call: Box::new(tool),
                    group: draft
                        .phase
                        .filter(|_| draft.grouped)
                        .map(|group| group.start),
                    tail: draft.phase.is_none_or(|group| group.last == draft.at),
                },
                message: draft.message,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            });
        } else if matches!(draft.role.as_str(), "assistant" | "reasoning") {
            self.rebuild_markdown(draft, building);
        } else {
            self.rebuild_plain(draft);
        }
    }

    fn index_rebuild_message(&mut self, draft: &RebuildMessage<'_>, building: &Rebuilding) {
        let mut prose_ordinal = 16;
        for row in &mut self.rows[draft.first..] {
            if draft.role == "reasoning" && matches!(row.body, Body::Reasoning | Body::Markdown(_))
            {
                row.thought_phase = draft
                    .phase
                    .filter(|_| draft.grouped)
                    .map(|group| (group.start, group.last == draft.at));
            }
            let ordinal = match row.body {
                Body::BotHeader { .. } => 0,
                Body::WorkLine { .. } => usize::from(self.bot),
                Body::Phase { .. } => 1 + usize::from(self.bot),
                Body::Tool { .. } => 2 + usize::from(self.bot),
                Body::Reasoning => 4,
                _ => {
                    let key = prose_ordinal;
                    prose_ordinal += 1;
                    key
                }
            };
            row.key = (draft.at, ordinal);
            row.work = building.has_work
                && building.ready
                && !draft.is_prompt
                && draft.role != "visual"
                && !building.activity.pinned.contains(&draft.at)
                && building
                    .final_answer
                    .is_some_and(|answer| draft.at < answer)
                && !matches!(
                    row.body,
                    Body::WorkLine { .. } | Body::BotHeader { .. } | Body::BotCard(_)
                );
        }
        if let Some(row) = self.rows.get_mut(draft.first) {
            row.first = true;
        }
        if self.rows.len() > draft.first {
            self.rows.last_mut().unwrap().last = true;
        }
    }

    fn finish_rebuild_message(
        &mut self,
        message: RebuildMessage<'_>,
        building: &Rebuilding,
    ) -> usize {
        self.index_rebuild_message(&message, building);
        let RebuildMessage {
            at,
            end,
            role,
            parser,
            message,
            is_prompt,
            reaction,
            attachments,
            peer_origin,
            ..
        } = message;
        self.messages.push(Message {
            start: at,
            display_start: None,
            role,
            parser,
            under_line: building.has_work && building.final_answer == Some(at),
            run: building.turn,
            turn: if self.bot {
                self.bot_group_start(building.turn)
            } else {
                building.turn
            },
            attachments,
            peer: peer_origin,
        });
        if is_prompt && building.last_prompt.is_some_and(|p| end > p) {
            // The running turn keeps the user's messages together above its work.
            self.push_pending(at);
        }
        self.rows.extend(building.cards.rows(
            at,
            building.turn,
            !is_prompt && end == building.turn_end,
            message,
        ));
        if !self.document
            && !self.source[at].is_prompt()
            && self.source[at].role != "bot-card"
            && self.source[at].role != vorkspace_core::transcript::attachments::ROLE
            && !reaction
            && end == building.turn_end
            && !(self.bot
                && self
                    .source
                    .get(building.turn_end)
                    .is_some_and(|r| r.role == "bot-system")
                && !self.bot_opens_stretch(building.turn_end))
        {
            if !self.bot
                && self.review_shown
                && self.review.is_some()
                && building.turn_end == self.source.len()
            {
                self.rows.push(UiRow {
                    key: (at, usize::MAX - 2),
                    body: Body::Review {
                        turn: building.turn,
                    },
                    message,
                    first: false,
                    last: false,
                    work: false,
                    work_tail: false,
                    thought_phase: None,
                });
            }
            self.rows.push(UiRow {
                key: (at, usize::MAX - 1),
                body: Body::Actions {
                    turn: if self.bot {
                        self.bot_group_start(building.turn)
                    } else {
                        building.turn
                    },
                    complete: !self.streaming || end < self.source.len(),
                },
                message,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            });
        }

        end
    }

    fn rebuild_pinned_rows(&mut self, building: &Rebuilding) -> HashMap<usize, usize> {
        let mut pinned_answers = HashMap::new();
        if !building.activity.pinned.is_empty() {
            let mut parked: HashMap<usize, Vec<UiRow>> = HashMap::new();
            let mut ordered = Vec::new();
            for row in self.rows.drain(building.display_start..) {
                // Pending messages and saved drafts have no history message.
                // Resolve a run only for the tool rows that can be pinned.
                if matches!(row.body, Body::Tool { .. })
                    && building.activity.pinned.contains(&row.key.0)
                    && let Some(answer) = self
                        .messages
                        .get(row.message)
                        .and_then(|message| self.fold_targets.get(&message.run))
                        .copied()
                        .flatten()
                        .filter(|answer| row.key.0 < *answer)
                {
                    pinned_answers.insert(row.key.0, answer);
                    parked.entry(answer).or_default().push(row);
                } else {
                    let mut due: Vec<_> = parked
                        .keys()
                        .copied()
                        .filter(|answer| *answer <= row.key.0)
                        .collect();
                    due.sort_unstable();
                    for answer in due {
                        ordered.extend(parked.remove(&answer).unwrap());
                    }
                    ordered.push(row);
                }
            }
            // A folded answer is always present; retain runs defensively if
            // a transient source patch has not made its prose row yet.
            let mut remaining: Vec<_> = parked.into_iter().collect();
            remaining.sort_by_key(|(answer, _)| *answer);
            for (_, runs) in remaining {
                ordered.extend(runs);
            }
            self.rows.extend(ordered);
        }

        pinned_answers
    }

    fn reconcile_rebuilt_rows(
        &mut self,
        building: &mut Rebuilding,
        pinned_answers: &HashMap<usize, usize>,
    ) -> Vec<UiRow> {
        // A phase and its nested steps are one sibling in the outer work tree.
        // Its final outer branch ends at the header, above the inner tool rail.
        let mut next_work_sibling = false;
        for row in self.rows[building.display_start..].iter_mut().rev() {
            row.work_tail = row.work && !next_work_sibling;
            if !row.in_phase() {
                next_work_sibling = row.work;
            }
        }
        let target = self.rows.split_off(building.display_start);
        if building.streaming_prose {
            // A growing answer replaces its presentation rows, but earlier
            // paragraphs can still use the same height at this width.
            self.measurements
                .rebuild_tail(building.source_start, &building.old_rows, &target);
        } else {
            self.measurements.clear();
        }
        if self.visible {
            let replaced_work = self
                .fold_targets
                .get(&building.current_turn)
                .copied()
                .flatten()
                .filter(|_| {
                    !self
                        .work_folds
                        .get(&building.current_turn)
                        .copied()
                        .unwrap_or(false)
                })
                .filter(|_| building.layout_changed)
                .map(|answer| building.current_turn..answer);
            if replaced_work.is_some() {
                // A replacement establishes a new block's baseline. The old
                // block's removed height is not a scroll animation to replay.
                self.follow.measured = None;
            }
            self.reconcile(
                std::mem::take(&mut building.old_rows),
                target,
                building.now,
                pinned_answers,
                replaced_work,
            )
        } else {
            self.motions.clear();
            target
        }
    }

    fn index_rebuilt_rows(&mut self, display_start: usize, merged: Vec<UiRow>) {
        self.phase_toggle = None;
        self.rows.extend(merged);
        self.measurements.rebuilt(self.rows.len());
        // The preserved prefix cannot acquire a windowed phase from a suffix
        // update unless it already owned a window or scroll state.
        let phase_start = if self.windowed_phases.is_empty() && self.phase_scrolls.is_empty() {
            display_start
        } else {
            0
        };
        self.windowed_phases = self.rows[phase_start..]
            .iter()
            .filter_map(|row| match row.body {
                Body::Phase { group, active, .. }
                    if active
                        || (self.phase_scrolls.contains_key(&group)
                            && !self.phases.get(&group).copied().unwrap_or(active)) =>
                {
                    Some(group)
                }
                _ => None,
            })
            .collect();
        // A closing live phase keeps its bounded window for the exit. Once
        // settled, an explicitly opened group returns to virtualized rows.
        for (&group, scroll) in &mut self.phase_scrolls {
            let enabled = self.visible && self.rows.iter().any(|row| {
                        matches!(row.body, Body::Phase { group: id, active: true, .. } if id == group)
                    }) && self.phases.get(&group).copied().unwrap_or(true);
            scroll.enable(enabled);
        }
        self.index_messages_from(display_start);
    }

    fn restore_rebuild_scroll(&mut self, building: &Rebuilding) {
        // Preserve surviving indices and scroll offsets while the tail reflows.
        let common = building.old_count.min(self.rows.len());
        if building.display_start < common {
            self.list.remeasure_items(building.display_start..common);
        }
        if building.old_count != self.rows.len() {
            self.list
                .splice(common..building.old_count, self.rows.len() - common);
        }
        self.list
            .remeasure_items(self.footer_ix()..self.footer_ix() + 1);
        if self.bot && building.display_start > 0 {
            // A new message can stack the one before it, changing its spacing.
            self.list.remeasure_items(
                building.display_start.saturating_sub(8)..building.display_start.min(common),
            );
        }
        if !self.hover_rows.is_empty() {
            let keys: HashSet<_> = self.rows.iter().map(|r| r.key).collect();
            self.hover_rows.retain(|key, _| keys.contains(key));
        }
        if building.following {
            self.list.scroll_to_end();
        } else if let Some(key) = building.anchor
            && let Some(item_ix) = (building.offset.item_ix < building.display_start)
                .then_some(building.offset.item_ix)
                .or_else(|| {
                    self.rows[building.display_start..]
                        .iter()
                        .position(|r| r.key == key)
                        .map(|ix| building.display_start + ix)
                })
        {
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: building.offset.offset_in_item,
            });
        }
    }

    fn finish_rebuild(&mut self, mut building: Rebuilding, cx: &mut Context<Self>) {
        self.prepared_parsers.clear();
        if self.bot && !self.rows.iter().any(|r| matches!(r.body, Body::Pending(_))) {
            self.push_pending(self.source.len());
        }
        self.push_saved_draft();

        let pinned_answers = self.rebuild_pinned_rows(&building);
        let merged = self.reconcile_rebuilt_rows(&mut building, &pinned_answers);
        self.index_rebuilt_rows(building.display_start, merged);
        self.restore_rebuild_scroll(&building);
        cx.notify();
    }
    pub(super) fn invalidate_message_indices(&mut self, start: usize) {
        for row in &self.rows[start..] {
            if let Some(message) = self.messages.get_mut(row.message)
                && message.display_start.is_some_and(|at| at >= start)
            {
                message.display_start = None;
            }
        }
    }

    pub(super) fn index_messages_from(&mut self, start: usize) {
        for (ix, row) in self.rows.iter().enumerate().skip(start) {
            if let Some(message) = self.messages.get_mut(row.message) {
                message.display_start.get_or_insert(ix);
            }
        }
    }
    pub(super) fn reconcile(
        &mut self,
        mut old: Vec<UiRow>,
        target: Vec<UiRow>,
        now: Instant,
        pinned_answers: &HashMap<usize, usize>,
        replaced_work: Option<std::ops::Range<usize>>,
    ) -> Vec<UiRow> {
        let phase_toggle = self.phase_toggle.take();
        if old.is_empty() && !self.streaming && self.motions.is_empty() {
            return target;
        }
        let order = |row: &UiRow| {
            if matches!(row.body, Body::Tool { .. })
                && let Some(&answer) = pinned_answers.get(&row.key.0)
            {
                (answer, 0, row.key.0, row.key.1)
            } else {
                (row.key.0, 1, row.key.0, row.key.1)
            }
        };
        // Source keys stay stable when runs are lifted below the work. The
        // answer boundary can advance between renders, so place outgoing
        // commentary in the fold above the pinned stack before merging it.
        old.sort_by_key(&order);
        let wanted: HashSet<_> = target.iter().map(|r| r.key).collect();
        let previous: HashMap<_, _> = old.iter().enumerate().map(|(ix, r)| (r.key, ix)).collect();
        let mut merged = Vec::new();
        let mut cursor = 0;
        let source = &self.source;
        let windowed = &self.windowed_phases;
        let keep_old = |row: &UiRow, motions: &mut HashMap<_, Motion>| -> bool {
            // "Show less" hides the rest of a message at once, as in React.
            if matches!(row.body, Body::Plain(_))
                && wanted.contains(&(row.key.0, 16))
                && source.get(row.key.0).is_some_and(|r| r.role == "user")
            {
                return false;
            }
            if matches!(
                row.body,
                Body::Actions { .. }
                    | Body::Pending(_)
                    | Body::Launch(_)
                    | Body::Review { .. }
                    | Body::SavedDraft
            ) {
                return false;
            }
            if wanted.contains(&row.key) {
                return false;
            }
            // A resource can acquire a new source anchor after replacement.
            // Retaining its obsolete representation for an exit would briefly
            // show both cards, unlike the atomic replacement of launch rows.
            if row.key.1 >= usize::MAX / 2 && matches!(row.body, Body::BotCard(_)) {
                return false;
            }
            // React replaces the live prose/work block when its answer moves
            // on. Keeping both blocks in layout for an exit animation can
            // temporarily overflow an otherwise short anchored turn.
            // Explicit disclosure toggles still animate in place.
            if replaced_work
                .as_ref()
                .is_some_and(|range| range.contains(&row.key.0))
            {
                motions.remove(&row.key);
                return false;
            }
            let motion = motions.entry(row.key).or_insert_with(|| {
                if let Some(group) = row
                    .phase_group()
                    .filter(|g| Some(*g) == phase_toggle || windowed.contains(g))
                {
                    Motion::phase(group, false, now)
                } else {
                    Motion::new(false, false, now)
                }
            });
            if motion.opening {
                *motion = motion.redirect(false, now);
            }
            !motion.finished(now)
        };
        for row in target {
            // Outgoing work stays above the new final answer during its exit.
            // New rows have no previous index to anchor the old suffix to.
            while cursor < old.len() && order(&old[cursor]) < order(&row) {
                if keep_old(&old[cursor], &mut self.motions) {
                    merged.push(old[cursor].clone());
                }
                cursor += 1;
            }
            if let Some(&at) = previous.get(&row.key) {
                if at >= cursor {
                    for old in &old[cursor..at] {
                        if keep_old(old, &mut self.motions) {
                            merged.push(old.clone());
                        }
                    }
                    cursor = at + 1;
                }
                if self.motions.get(&row.key).is_some_and(|m| !m.opening) {
                    let motion = self.motions[&row.key].redirect(true, now);
                    self.motions.insert(row.key, motion);
                }
            } else {
                let unseen = if matches!(row.body, Body::Tool { .. }) {
                    self.seen_steps.insert(row.key.0)
                } else {
                    false
                };
                let phase = row.phase_group().filter(|g| Some(*g) == phase_toggle);
                let step = phase.is_none()
                    && self.streaming
                    && row.key.0 >= self.live_start
                    && unseen
                    && !row.work;
                if step
                    || matches!(row.body, Body::Tool { .. } | Body::Reasoning)
                    || self.expanded.contains(&row.key.0)
                    || row.work
                {
                    let mut motion = phase.map_or_else(
                        || Motion::new(true, step, now),
                        |group| Motion::phase(group, true, now),
                    );
                    if step {
                        let group = match row.body {
                            Body::Tool { group, .. } => group.unwrap_or(row.key.0),
                            _ => row.key.0,
                        };
                        let start = self.step_queue.get(&group).copied().unwrap_or(now).max(now);
                        let wait = start.duration_since(now).as_secs_f32();
                        let pace = (0.48 * ((2. - wait) / (2. - 0.96)).min(1.)).max(0.16);
                        motion.start = start;
                        motion.duration = std::time::Duration::from_secs_f32(pace);
                        self.step_queue.insert(group, start + motion.duration);
                    }
                    self.motions.insert(row.key, motion);
                }
            }
            merged.push(row);
        }
        for row in &old[cursor..] {
            if keep_old(row, &mut self.motions) {
                merged.push(row.clone());
            }
        }
        merged
    }
}
