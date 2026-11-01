//! Turning transcript rows into presentation rows, incrementally where possible.
use super::*;

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
            for prompt in known..self.source.len() {
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
            for ix in start.max(known)..self.source.len() {
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
        self.measurements.clear();
        #[cfg(test)]
        {
            self.rebuild_passes += 1;
        }
        self.phase_layouts.clear();
        let now = Instant::now();
        let following = self.list.is_following_tail();
        let offset = self.list.logical_scroll_top();
        let anchor = self.rows.get(offset.item_ix).map(|r| r.key);
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
        {
            let end = (at + 1..current_end)
                .find(|&i| self.source[i].first)
                .unwrap_or(current_end);
            let text: String = self.source[at..end]
                .iter()
                .map(|r| r.text.as_str())
                .collect();
            self.prepare_reveal(at, end, &text, true, now);
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
        let known_message = self.messages.iter().any(|m| {
            m.start <= start
                && m.role == "assistant"
                && self
                    .source
                    .get(start)
                    .is_some_and(|r| r.role == "assistant")
        });
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
        if self.source[current_turn..current_end].iter().any(|row| {
            row.role == "tool"
                && row.first
                && serde_json::from_str::<ToolCall>(&row.text)
                    .is_ok_and(|call| matches!(call.panel(), Some(Panel::Agent(_))))
        }) || self.rows.iter().any(|row| {
            self.messages
                .get(row.message)
                .is_some_and(|message| message.run == current_turn)
                && matches!(
                    &row.body,
                    Body::Tool { call, .. } if matches!(call.panel(), Some(Panel::Agent(_)))
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
        let source_start = if message_ix == 0 {
            self.history_start
        } else {
            self.messages[message_ix].start
        };
        let display_start = if message_ix == 0 {
            0
        } else {
            // A hidden opening message can inherit a pinned run's later
            // position. Cut before every displayed row in the source suffix,
            // including work that now appears above that run.
            self.messages[message_ix..]
                .iter()
                .map(|message| message.display_start)
                .min()
                .unwrap_or(self.rows.len())
        };
        let old_count = self.rows.len();
        let old_rows = self.rows[display_start..].to_vec();
        let previous_cards = cards::Previous::new(&old_rows, &self.messages);
        let mut previous: HashMap<_, _> = self
            .messages
            .split_off(message_ix)
            .into_iter()
            .map(|m| (m.start, m))
            .collect();
        self.rows.truncate(display_start);
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
        let mut at = source_start;
        let mut turn = 0;
        let mut turn_end = 0;
        let mut final_answer = None;
        let mut awaiting_answer = None;
        let mut has_work = false;
        let mut ready = false;
        let mut receipt_start = None;
        while at < self.source.len() {
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
            let mut parser = previous
                .remove(&at)
                .filter(|p| p.role == role)
                .map(|p| p.parser)
                .or_else(|| self.prepared_parsers.remove(&at))
                .unwrap_or_default();
            let message = self.messages.len();
            let first = self.rows.len();
            if at >= turn_end {
                turn = if is_prompt {
                    (at..self.source.len())
                        .find(|&i| !self.source[i].is_prompt())
                        .unwrap_or(self.source.len())
                } else {
                    self.source[..at]
                        .iter()
                        .rposition(|r| r.is_prompt())
                        .map_or(0, |i| i + 1)
                };
                turn_end = (turn..self.source.len())
                    .find(|&i| self.source[i].is_prompt())
                    .unwrap_or(self.source.len());
                awaiting_answer = (turn..turn_end)
                    .rev()
                    .find(|&i| self.source[i].role == "assistant" && self.source[i].first)
                    .filter(|&i| !self.answer_visible(i));
                final_answer = (turn..turn_end).rev().find(|&i| {
                    self.source[i].role == "assistant"
                        && self.source[i].first
                        && self.answer_visible(i)
                });
                has_work = final_answer.is_some_and(|answer| {
                    self.source[turn..answer].iter().any(|r| r.role == "tool")
                });
                ready = final_answer.is_some_and(|answer| self.answer_visible(answer));
                self.fold_targets
                    .insert(turn, final_answer.filter(|_| has_work && ready));
                receipt_start = (turn..turn_end).find(|&i| {
                    let row = &self.source[i];
                    row.first
                        && !matches!(
                            row.role.as_str(),
                            "bot-card"
                                | "remote-plan"
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
            let folded = !is_prompt
                && !activity.pinned.contains(&at)
                && has_work
                && ready
                && final_answer.is_some_and(|answer| at < answer)
                && !self.work_folds.get(&turn).copied().unwrap_or(false);
            if !self.document && receipt_start == Some(at) {
                if self.bot
                    && (self.bot_group_start(turn) == turn
                        || (self.history_start > 0
                            && self.prompt_start(turn) == self.history_start))
                {
                    self.rows.push(UiRow {
                        key: (0, 0),
                        body: Body::BotHeader {
                            turn: self.bot_group_start(turn),
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
                        turn,
                        expandable: has_work && ready,
                    },
                    message,
                    first: false,
                    last: false,
                    work: false,
                    work_tail: false,
                    thought_phase: None,
                });
            }
            let phase = activity.at(at);
            let grouped = phase.is_some_and(|group| group.steps > 1);
            let active = phase.is_some_and(|group| {
                self.streaming
                    && (group.end == self.source.len() || awaiting_answer == Some(group.end))
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
            if role == "interjection" {
                if let Ok(note) = serde_json::from_str(&text) {
                    self.rows.push(UiRow {
                        key: (0, 0),
                        body: Body::Interjection(note),
                        message,
                        first: false,
                        last: false,
                        work: false,
                        work_tail: false,
                        thought_phase: None,
                    });
                }
            } else if role == "remote-plan" {
                if let Ok(plan) = serde_json::from_str(&text) {
                    self.rows.push(UiRow {
                        key: (0, 0),
                        body: Body::RemotePlan(plan),
                        message,
                        first: false,
                        last: false,
                        work: false,
                        work_tail: false,
                        thought_phase: None,
                    });
                }
            } else if role == "visual" {
                let document =
                    serde_json::from_str::<vorkspace_core::visual::VisualDocument>(&text)
                        .ok()
                        .filter(|document| document.validate().is_ok());
                let body = match document {
                    Some(document) => Body::Visual(Arc::new(document)),
                    None => Body::Plain("This visual could not be loaded.".into()),
                };
                self.rows.push(UiRow {
                    key: (0, 0),
                    body,
                    message,
                    first: false,
                    last: false,
                    work: false,
                    work_tail: false,
                    thought_phase: None,
                });
            } else if matches!(role.as_str(), "bot-card" | "bot-note") {
                if !cards.owns(at)
                    && let Ok(value) = serde_json::from_str(&text)
                {
                    self.rows.push(UiRow {
                        key: (0, 0),
                        body: Body::BotCard(value),
                        message,
                        first: false,
                        last: false,
                        work: false,
                        work_tail: false,
                        thought_phase: None,
                    });
                }
            } else if stopped_notice
                || matches!(
                    role.as_str(),
                    "bot-document" | "bot-launch" | vorkspace_core::transcript::attachments::ROLE
                )
            {
                // Saved work references are rendered once under the settled reply.
            } else if reaction {
                // BotReactionBadge: the emoji sits on the message it answers.
                if let Some(emoji) = text
                    .trim()
                    .strip_prefix("<reaction>")
                    .and_then(|s| s.strip_suffix("</reaction>"))
                    .filter(|e| e.chars().count() <= 16)
                    && let Some(prompt) = self.source[..at].iter().rposition(|r| r.is_prompt())
                {
                    let prompt = self.source[..=prompt]
                        .iter()
                        .rposition(|r| r.first)
                        .unwrap_or(prompt);
                    let live = self.streaming && at >= self.live_start && self.visible;
                    match self.reactions.get(&prompt) {
                        Some((current, _)) if current == emoji => {}
                        _ => {
                            self.reactions
                                .insert(prompt, (emoji.into(), live.then_some(now)));
                        }
                    }
                }
            } else if folded || activity.hidden.contains(&at) || !phase_open {
                if role == "tool" {
                    self.seen_steps.insert(at);
                }
                // WorkFoldLine puts earlier commentary and work behind its receipt.
            } else if role == "tool"
                && let Ok(tool) = serde_json::from_str::<ToolCall>(&text)
            {
                self.rows.push(UiRow {
                    key: (0, 0),
                    body: Body::Tool {
                        call: Box::new(tool),
                        group: phase.filter(|_| grouped).map(|group| group.start),
                        tail: phase.is_none_or(|group| group.last == at),
                    },
                    message,
                    first: false,
                    last: false,
                    work: false,
                    work_tail: false,
                    thought_phase: None,
                });
            } else if matches!(role.as_str(), "assistant" | "reasoning") {
                if role == "reasoning" {
                    self.rows.push(UiRow {
                        key: (0, 0),
                        body: Body::Reasoning,
                        message,
                        first: false,
                        last: false,
                        work: false,
                        work_tail: false,
                        thought_phase: None,
                    });
                }
                self.prepare_reveal(at, end, &text, role == "assistant", now);
                let paced = self.reveals.get_mut(&at).unwrap();
                parser.set_text(paced.text());
                let tree = if paced.behind() || (self.streaming && end == self.source.len()) {
                    parser.display_tree()
                } else {
                    parser.tree().clone()
                };
                let mut word_cursor = 0;
                for block in tree
                    .blocks
                    .iter()
                    .filter(|_| role != "reasoning" || self.expanded.contains(&at))
                {
                    self.rows
                        .extend(markdown::rows(&block.block).into_iter().map(|mut row| {
                            row.word_start = word_cursor;
                            word_cursor += markdown::word_count(&row);
                            UiRow {
                                key: (0, 0),
                                body: Body::Markdown(row),
                                message,
                                first: false,
                                last: false,
                                work: false,
                                work_tail: false,
                                thought_phase: None,
                            }
                        }));
                }
                paced.words(word_cursor, now);
            } else {
                let text_rows = if let Some(text) = display_text {
                    let mut transcript = vorkspace_core::transcript::Transcript::default();
                    transcript.begin("user");
                    transcript.append(&text);
                    transcript
                        .rows
                        .into_iter()
                        .map(|row| row.text)
                        .collect::<Vec<_>>()
                } else {
                    self.source[at..end]
                        .iter()
                        .filter(|r| r.role != "bot-system")
                        .map(|row| row.text.clone())
                        .collect()
                };
                // `line-clamp-4`: a collapsed message shows its first lines only.
                let clamped =
                    role == "user" && !self.expanded_prompts.contains(&at) && text_rows.len() > 1;
                if clamped {
                    self.clamped_prompts.insert(at);
                } else {
                    self.clamped_prompts.remove(&at);
                }
                let shown = if clamped { 1 } else { text_rows.len() };
                for text in text_rows.into_iter().take(shown) {
                    self.rows.push(UiRow {
                        key: (0, 0),
                        body: Body::Plain(text.into()),
                        message,
                        first: false,
                        last: false,
                        work: false,
                        work_tail: false,
                        thought_phase: None,
                    });
                }
            }
            let mut prose_ordinal = 16;
            for row in &mut self.rows[first..] {
                if role == "reasoning" && matches!(row.body, Body::Reasoning | Body::Markdown(_)) {
                    row.thought_phase = phase
                        .filter(|_| grouped)
                        .map(|group| (group.start, group.last == at));
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
                row.key = (at, ordinal);
                row.work = has_work
                    && ready
                    && !is_prompt
                    && role != "visual"
                    && !activity.pinned.contains(&at)
                    && final_answer.is_some_and(|answer| at < answer)
                    && !matches!(
                        row.body,
                        Body::WorkLine { .. } | Body::BotHeader { .. } | Body::BotCard(_)
                    );
            }
            if let Some(row) = self.rows.get_mut(first) {
                row.first = true;
            }
            if self.rows.len() > first {
                self.rows.last_mut().unwrap().last = true;
            }
            self.messages.push(Message {
                start: at,
                display_start: first,
                role,
                parser,
                under_line: has_work && final_answer == Some(at),
                run: turn,
                turn: if self.bot {
                    self.bot_group_start(turn)
                } else {
                    turn
                },
                attachments,
                peer: peer_origin,
            });
            let last_prompt = self.source.iter().rposition(|r| r.is_prompt());
            if self.bot && is_prompt && self.streaming && last_prompt.is_some_and(|p| end > p) {
                // The running turn keeps the user's messages together above its work.
                self.push_pending(at);
            }
            self.rows
                .extend(cards.rows(at, turn, !is_prompt && end == turn_end, message));
            if !self.document
                && !self.source[at].is_prompt()
                && self.source[at].role != "bot-card"
                && self.source[at].role != vorkspace_core::transcript::attachments::ROLE
                && !reaction
                && end == turn_end
                && !(self.bot
                    && self
                        .source
                        .get(turn_end)
                        .is_some_and(|r| r.role == "bot-system")
                    && !self.bot_opens_stretch(turn_end))
            {
                if !self.bot
                    && self.review_shown
                    && self.review.is_some()
                    && turn_end == self.source.len()
                {
                    self.rows.push(UiRow {
                        key: (at, usize::MAX - 2),
                        body: Body::Review { turn },
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
                            self.bot_group_start(turn)
                        } else {
                            turn
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
            at = end;
        }
        self.prepared_parsers.clear();
        if self.bot && !self.rows.iter().any(|r| matches!(r.body, Body::Pending(_))) {
            self.push_pending(self.source.len());
        }
        self.push_saved_draft();
        let mut pinned_answers = HashMap::new();
        if !activity.pinned.is_empty() {
            let mut parked: HashMap<usize, Vec<UiRow>> = HashMap::new();
            let mut ordered = Vec::new();
            for row in self.rows.drain(display_start..) {
                let answer = self
                    .fold_targets
                    .get(&self.messages[row.message].run)
                    .copied()
                    .flatten();
                if activity.pinned.contains(&row.key.0)
                    && matches!(row.body, Body::Tool { .. })
                    && let Some(answer) = answer.filter(|answer| row.key.0 < *answer)
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
        // A phase and its nested steps are one sibling in the outer work tree.
        // Its final outer branch ends at the header, above the inner tool rail.
        let mut next_work_sibling = false;
        for row in self.rows[display_start..].iter_mut().rev() {
            row.work_tail = row.work && !next_work_sibling;
            if !row.in_phase() {
                next_work_sibling = row.work;
            }
        }
        let target = self.rows.split_off(display_start);
        let merged = if self.visible {
            let replaced_work = self
                .fold_targets
                .get(&current_turn)
                .copied()
                .flatten()
                .filter(|_| !self.work_folds.get(&current_turn).copied().unwrap_or(false))
                .filter(|_| layout_changed)
                .map(|answer| current_turn..answer);
            if replaced_work.is_some() {
                // A replacement establishes a new block's baseline. The old
                // block's removed height is not a scroll animation to replay.
                self.follow.measured = None;
            }
            self.reconcile(old_rows, target, now, &pinned_answers, replaced_work)
        } else {
            self.motions.clear();
            target
        };
        self.phase_toggle = None;
        self.rows.extend(merged);
        self.windowed_phases = self
            .rows
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
        self.index_messages();
        // Preserve surviving indices and scroll offsets while the tail reflows.
        let common = old_count.min(self.rows.len());
        if display_start < common {
            self.list.remeasure_items(display_start..common);
        }
        if old_count != self.rows.len() {
            self.list
                .splice(common..old_count, self.rows.len() - common);
        }
        self.list
            .remeasure_items(self.footer_ix()..self.footer_ix() + 1);
        if self.bot && display_start > 0 {
            // A new message can stack the one before it, changing its spacing.
            self.list
                .remeasure_items(display_start.saturating_sub(8)..display_start.min(common));
        }
        if !self.hover_rows.is_empty() {
            let keys: HashSet<_> = self.rows.iter().map(|r| r.key).collect();
            self.hover_rows.retain(|key, _| keys.contains(key));
        }
        if following {
            self.list.scroll_to_end();
        } else if let Some(key) = anchor
            && let Some(item_ix) = self.rows.iter().position(|r| r.key == key)
        {
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: offset.offset_in_item,
            });
        }
        cx.notify();
    }
    pub(super) fn prepare_reveal(
        &mut self,
        at: usize,
        end: usize,
        text: &str,
        assistant: bool,
        now: Instant,
    ) {
        let paced = self.reveals.entry(at).or_insert_with(|| {
            PacedText::new(
                text,
                self.visible
                    && self.smooth_reveal
                    && self.streaming
                    && at >= self.live_start
                    && assistant,
                now,
            )
        });
        paced.update(text, now);
        if !self.visible || !self.smooth_reveal {
            paced.settle(now);
        }
        paced.step(now, self.streaming && end == self.source.len());
    }
    pub(super) fn answer_visible(&self, answer: usize) -> bool {
        if self.source[answer].text.trim().is_empty() {
            return false;
        }
        if answer < self.live_start || !self.visible || !self.smooth_reveal {
            return true;
        }
        self.reveals
            .get(&answer)
            .map_or(!self.streaming, |r| !r.text().trim().is_empty())
    }
    pub(super) fn index_messages(&mut self) {
        let mut first = HashMap::new();
        for (ix, row) in self.rows.iter().enumerate() {
            first.entry(row.message).or_insert(ix);
        }
        let mut next = self.rows.len();
        for (ix, message) in self.messages.iter_mut().enumerate().rev() {
            if let Some(at) = first.get(&ix) {
                next = *at;
            }
            message.display_start = next;
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
