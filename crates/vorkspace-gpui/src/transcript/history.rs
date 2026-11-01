//! Compact interaction state survives an inactive transcript cache eviction.
use super::*;

const PAGE_TURNS: usize = 10;

pub(super) struct StorageHistory {
    store: vorkspace_runtime::runtime::Database,
    session: i64,
    origin: usize,
    cursor: Option<vorkspace_core::store::IndexedHistoryCursor>,
    turns: HashMap<usize, vorkspace_core::agent::TurnInfo>,
    search: Option<(usize, String)>,
    error: Option<String>,
}

/// Keep source row indices intact for persistence, tools and live patches.
/// Only the displayed window needs Markdown and presentation objects.
pub(crate) struct PreparedHistory {
    rows: Vec<Row>,
    parsers: HashMap<usize, MarkdownParser>,
    start: usize,
}
impl PreparedHistory {
    /// Run on the history loader, before handing its result to the UI.
    pub(crate) fn new(rows: Vec<Row>, bot: bool) -> Self {
        let start = if bot {
            page_start(&rows, rows.len())
        } else {
            0
        };
        let parsers = prepare_parsers(&rows[start..], start);
        Self {
            rows,
            parsers,
            start,
        }
    }
}

pub(super) fn page_start(rows: &[Row], before: usize) -> usize {
    rows[..before.min(rows.len())]
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, row)| row.first && row.is_prompt())
        .nth(PAGE_TURNS - 1)
        .map_or(0, |(at, _)| at)
}

fn prepare_parsers(rows: &[Row], offset: usize) -> HashMap<usize, MarkdownParser> {
    let mut parsers = HashMap::new();
    let mut at = 0;
    while at < rows.len() {
        let end = (at + 1..rows.len())
            .find(|&ix| rows[ix].first)
            .unwrap_or(rows.len());
        if matches!(rows[at].role.as_str(), "assistant" | "reasoning") {
            let text: String = rows[at..end].iter().map(|row| row.text.as_str()).collect();
            let mut parser = MarkdownParser::new();
            parser.set_text(&text);
            parsers.insert(offset + at, parser);
        }
        at = end;
    }
    parsers
}

pub(super) struct HistoryPosition {
    key: Option<(usize, usize)>,
    offset: ListOffset,
    following: bool,
}
impl TranscriptView {
    pub(crate) fn install_storage_history(
        &mut self,
        page: vorkspace_core::store::IndexedHistoryPage,
        prepared: PreparedHistory,
        store: vorkspace_runtime::runtime::Database,
        session: i64,
        cx: &mut Context<Self>,
    ) {
        let origin = page.global_start;
        let turns = page.turns;
        self.replace_prepared_history(prepared, cx);
        self.turns = turns
            .iter()
            .filter_map(|(&at, info)| at.checked_sub(origin).map(|at| (at, info.clone())))
            .collect();
        self.storage_history = Some(StorageHistory {
            store,
            session,
            origin,
            cursor: page.before,
            turns,
            search: None,
            error: None,
        });
        self.rebuild(0, cx);
    }

    pub(crate) fn storage_origin(&self) -> Option<usize> {
        self.storage_history.as_ref().map(|state| state.origin)
    }
    pub(crate) fn take_storage_search(&mut self) -> Option<(usize, String)> {
        self.storage_history
            .as_mut()
            .and_then(|state| state.search.take())
    }

    pub(super) fn storage_has_older(&self) -> bool {
        self.storage_history
            .as_ref()
            .is_some_and(|state| state.cursor.is_some())
    }

    pub(crate) fn reveal_storage_search(
        &mut self,
        row: usize,
        query: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.storage_history.as_mut() else {
            return;
        };
        if let Some(local) = row.checked_sub(state.origin) {
            if local < self.source.len() {
                self.reveal_search(local, query, cx);
            } else {
                state.search = Some((row, query.to_owned()));
                cx.emit(StorageHistoryStale);
            }
        } else if state.cursor.is_some() {
            state.search = Some((row, query.to_owned()));
            self.load_older_history(cx);
        }
    }

    pub(crate) fn complete_storage_history(
        &mut self,
        mut history: PreparedHistory,
        cx: &mut Context<Self>,
    ) {
        let origin = self.storage_history.take().map_or(0, |state| state.origin);
        history.start = history.start.min(origin);
        let offset = self.list.logical_scroll_top();
        let position = HistoryPosition {
            key: self
                .rows
                .get(offset.item_ix)
                .map(|row| (row.key.0.saturating_add(origin), row.key.1)),
            offset,
            following: self.list.is_following_tail(),
        };
        self.replace_rebased_history(history, origin, cx);
        self.restore_reading_position(position);
    }

    pub(super) fn replace_rebased_history(
        &mut self,
        history: PreparedHistory,
        shift: usize,
        cx: &mut Context<Self>,
    ) {
        fn set(values: HashSet<usize>, shift: usize) -> HashSet<usize> {
            values
                .into_iter()
                .map(|at| at.saturating_add(shift))
                .collect()
        }
        fn map<T>(values: HashMap<usize, T>, shift: usize) -> HashMap<usize, T> {
            values
                .into_iter()
                .map(|(at, value)| (at.saturating_add(shift), value))
                .collect()
        }
        let selection = std::mem::take(&mut self.selection);
        selection.shift_source(shift);
        self.agent_panels.shift_source(shift);
        let expanded = set(std::mem::take(&mut self.expanded), shift);
        let prompts = set(std::mem::take(&mut self.expanded_prompts), shift);
        let clamped = set(std::mem::take(&mut self.clamped_prompts), shift);
        let phases = map(std::mem::take(&mut self.phases), shift);
        let phase_scrolls = map(std::mem::take(&mut self.phase_scrolls), shift);
        let folds = map(std::mem::take(&mut self.work_folds), shift);
        let windowed = set(std::mem::take(&mut self.windowed_phases), shift);
        let elapsed = map(std::mem::take(&mut self.elapsed), shift);
        let seen_steps = set(std::mem::take(&mut self.seen_steps), shift);
        let step_queue = map(std::mem::take(&mut self.step_queue), shift);
        let fold_targets = std::mem::take(&mut self.fold_targets)
            .into_iter()
            .map(|(at, target)| {
                (
                    at.saturating_add(shift),
                    target.map(|at| at.saturating_add(shift)),
                )
            })
            .collect();
        let search = self
            .search_match
            .take()
            .map(|(at, query)| (at.saturating_add(shift), query));
        self.replace_prepared_history(history, cx);
        self.selection = selection;
        self.expanded = expanded;
        self.expanded_prompts = prompts;
        self.clamped_prompts = clamped;
        self.phases = phases;
        self.phase_scrolls = phase_scrolls;
        self.work_folds = folds;
        self.windowed_phases = windowed;
        self.elapsed = elapsed;
        self.seen_steps = seen_steps;
        self.step_queue = step_queue;
        self.fold_targets = fold_targets;
        self.search_match = search;
        self.target_action = None;
        self.rebuild(0, cx);
    }

    pub(crate) fn install_prepared_history(
        &mut self,
        history: PreparedHistory,
        cx: &mut Context<Self>,
    ) {
        if history.rows.len() < self.source.len() {
            let offset = self.list.logical_scroll_top();
            let position = HistoryPosition {
                key: self.rows.get(offset.item_ix).map(|row| row.key),
                offset,
                following: self.list.is_following_tail(),
            };
            let common = self
                .source
                .iter()
                .zip(&history.rows)
                .take_while(|(before, after)| before == after)
                .count();
            // A continuation belongs to its entire message: truncating one
            // chunk invalidates selections within that message as well.
            let stable = if self.source.get(common).is_some_and(|row| !row.first) {
                self.source[..common]
                    .iter()
                    .rposition(|row| row.first)
                    .unwrap_or(0)
            } else {
                common
            };
            self.install_shorter_history(history, stable, position, cx);
            return;
        }
        self.prepared_parsers = history.parsers;
        if history.rows.is_empty() {
            // Provider history can be empty while a locally queued message
            // or saved draft still owns a presentation row.
            self.rebuild(0, cx);
        } else {
            self.patch_with_page(0, history.rows, Some(history.start), cx);
        }
    }

    /// A projected snapshot owns its end as well as its changed suffix.
    /// Ordinary provider patches remain append/replace operations.
    pub(crate) fn patch_snapshot(
        &mut self,
        start: usize,
        rows: Vec<Row>,
        source_len: usize,
        cx: &mut Context<Self>,
    ) {
        assert_eq!(
            source_len,
            start + rows.len(),
            "snapshot tail must reach its authoritative end"
        );
        if source_len >= self.source.len() {
            self.patch(start, rows, cx);
            return;
        }
        assert!(
            start <= self.source.len(),
            "snapshot patches must be contiguous"
        );
        let offset = self.list.logical_scroll_top();
        let position = HistoryPosition {
            key: self.rows.get(offset.item_ix).map(|row| row.key),
            offset,
            following: self.list.is_following_tail(),
        };
        let common = start
            + self.source[start..]
                .iter()
                .zip(&rows)
                .take_while(|(before, after)| before == after)
                .count();
        let stable = if self.source.get(common).is_some_and(|row| !row.first) {
            self.source[..common]
                .iter()
                .rposition(|row| row.first)
                .unwrap_or(0)
        } else {
            common
        };
        // Move the surviving prefix and reuse its parsers. A compacted host
        // history must not copy or parse all unchanged text on the UI thread.
        let mut retained = std::mem::take(&mut self.source);
        retained.truncate(start);
        retained.extend(rows);
        let start = if self.bot {
            page_start(&retained, retained.len()).min(self.history_start)
        } else {
            0
        };
        let history = PreparedHistory {
            rows: retained,
            parsers: HashMap::new(),
            start,
        };
        self.install_shorter_history(history, stable, position, cx);
    }

    fn install_shorter_history(
        &mut self,
        mut history: PreparedHistory,
        stable: usize,
        position: HistoryPosition,
        cx: &mut Context<Self>,
    ) {
        // Preserve an explicitly opened older page. Its existing parsers
        // can follow the replacement without reparsing that page on the
        // interactive thread or collapsing it back to the recent tail.
        history.start = history.start.min(self.history_start);
        for message in &mut self.messages {
            if (history.start..history.rows.len()).contains(&message.start) {
                history.parsers.entry(message.start).or_insert_with(|| {
                    std::mem::replace(&mut message.parser, MarkdownParser::new())
                });
            }
        }
        let selection = std::mem::take(&mut self.selection);
        self.replace_prepared_history(history, cx);
        selection.retain_source_prefix(stable);
        self.selection = selection;
        self.restore_reading_position(position);
    }

    pub(super) fn older_history_row(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .id("older-bot-history")
            .debug_selector(|| "older-bot-history".into())
            .role(Role::Button)
            .aria_label("Load earlier messages")
            .w_full()
            .py_3()
            .flex()
            .justify_center()
            .text_size(px(12.))
            .text_color(content(0.5))
            .cursor_pointer()
            .hover(|s| s.text_color(content(0.8)))
            .child(if self.history_loading {
                "Loading earlier messages…"
            } else if self
                .storage_history
                .as_ref()
                .is_some_and(|state| state.error.is_some())
            {
                "Could not load earlier messages. Click to retry."
            } else {
                "Load earlier messages"
            })
            .on_click(cx.listener(|this, _, _, cx| this.load_older_history(cx)))
            .into_any_element()
    }

    pub(super) fn load_older_history(&mut self, cx: &mut Context<Self>) {
        if self.storage_history.is_some() {
            self.load_older_storage(cx);
            return;
        }
        if self.history_start == 0 || self.history_loading {
            return;
        }
        self.history_loading = true;
        self.list.pause_following_tail();
        let previous = self.history_start;
        let before = self
            .history_search
            .as_ref()
            .map_or(previous, |(row, _)| row.saturating_add(1).min(previous));
        let start = page_start(&self.source, before);
        // Snapshot only this page. A render/scroll callback must never copy
        // the long-lived conversation or parse its Markdown synchronously.
        let rows = self.source[start..previous].to_vec();
        let epoch = self.highlight_epoch;
        let work = cx
            .background_executor()
            .spawn(async move { prepare_parsers(&rows, start) });
        cx.spawn(async move |this, cx| {
            let parsers = work.await;
            let _ = this.update(cx, |this, cx| {
                if epoch != this.highlight_epoch || this.history_start != previous {
                    return;
                }
                this.history_loading = false;
                this.history_start = start;
                this.prepared_parsers.extend(parsers);
                this.rebuild(start, cx);
                if let Some((row, query)) = this.history_search.take() {
                    this.reveal_search(row, &query, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn load_older_storage(&mut self, cx: &mut Context<Self>) {
        if self.history_loading {
            return;
        }
        let Some(state) = self.storage_history.as_mut() else {
            return;
        };
        let Some(cursor) = state.cursor.clone() else {
            return;
        };
        state.error = None;
        let origin = state.origin;
        let store = state.store.clone();
        let session = state.session;
        let epoch = self.highlight_epoch;
        self.history_loading = true;
        self.list.pause_following_tail();
        let work = cx.background_executor().spawn(async move {
            let page = vorkspace_runtime::runtime::locked(&store)?
                .indexed_history_page(session, Some(&cursor), PAGE_TURNS)?
                .ok_or_else(|| "Indexed history is no longer available".to_owned())?;
            let parsers = prepare_parsers(&page.rows, 0);
            Ok::<_, String>((page, parsers))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.highlight_epoch != epoch || this.storage_origin() != Some(origin) {
                    return;
                }
                this.history_loading = false;
                let (page, mut parsers) = match result {
                    Ok(page) => page,
                    Err(error) => {
                        let stale =
                            error.contains("changed") || error.contains("no longer available");
                        if let Some(state) = this.storage_history.as_mut() {
                            state.error = Some(error);
                        }
                        if stale {
                            cx.emit(StorageHistoryStale);
                        }
                        cx.notify();
                        return;
                    }
                };
                if page.global_start.checked_add(page.rows.len()) != Some(origin) {
                    if let Some(state) = this.storage_history.as_mut() {
                        state.error = Some("History changed; reload before retrying".into());
                    }
                    cx.emit(StorageHistoryStale);
                    cx.notify();
                    return;
                }
                let added = page.rows.len();
                let offset = this.list.logical_scroll_top();
                let position = HistoryPosition {
                    key: this
                        .rows
                        .get(offset.item_ix)
                        .map(|row| (row.key.0.saturating_add(added), row.key.1)),
                    offset,
                    following: false,
                };
                let mut state = this.storage_history.take().unwrap();
                state.origin = page.global_start;
                state.cursor = page.before;
                state.turns.extend(page.turns);
                let mut rows = page.rows;
                rows.extend(std::mem::take(&mut this.source));
                for message in &mut this.messages {
                    parsers
                        .entry(message.start.saturating_add(added))
                        .or_insert_with(|| {
                            std::mem::replace(&mut message.parser, MarkdownParser::new())
                        });
                }
                this.replace_rebased_history(
                    PreparedHistory {
                        rows,
                        parsers,
                        start: 0,
                    },
                    added,
                    cx,
                );
                this.turns = state
                    .turns
                    .iter()
                    .filter_map(|(&at, info)| {
                        at.checked_sub(state.origin).map(|at| (at, info.clone()))
                    })
                    .collect();
                let search = state.search.take();
                this.storage_history = Some(state);
                this.rebuild(0, cx);
                this.restore_reading_position(position);
                if let Some((row, query)) = search {
                    this.reveal_storage_search(row, &query, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn replace_history(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        self.replace_prepared_history(PreparedHistory::new(rows, self.bot), cx);
    }

    fn replace_prepared_history(&mut self, history: PreparedHistory, cx: &mut Context<Self>) {
        let len = history.rows.len();
        self.selection.clear();
        self.history_position = None;
        self.history_loading = false;
        self.history_requested = false;
        self.history_search = None;
        self.prepared_parsers.clear();
        self.bot_groups.get_mut().clear();
        self.active_provider_turn_id = None;
        self.search_match = None;
        self.anchor_turn = None;
        self.prompt_rise = None;
        self.prompt_rise_pending = false;
        self.introduced_prompt = None;
        self.turns.retain(|&start, _| start < len);
        self.elapsed.retain(|&start, _| start < len);
        self.expanded.retain(|&start| start < len);
        self.phases.retain(|&start, _| start < len);
        self.phase_layouts.clear();
        self.attachment_images.clear();
        self.visuals.clear();
        self.reveals.clear();
        self.motions.clear();
        self.entrances.clear();
        self.hover_rows.clear();
        self.hovered_turn = None;
        self.hovered_prompt = None;
        self.hovered_prompt_actions = None;
        self.outline = Default::default();
        self.reflow = Default::default();
        self.follow = Default::default();
        self.highlight_epoch = self.highlight_epoch.wrapping_add(1);
        self.highlights.clear();
        self.highlighting.clear();
        self.highlight_queue.clear();
        self.highlight_active = false;
        self.highlight_order.clear();
        self.highlight_bytes = 0;
        self.restore_prepared_history(history, cx);
    }
    /// A conservative payload estimate, including the source, parsed markdown
    /// and presentation copies. The inactive cache also has a session count cap.
    pub(crate) fn history_weight(&self) -> usize {
        self.source_bytes
            .saturating_mul(4)
            .saturating_add(
                self.source
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Row>()),
            )
            .saturating_add(self.highlight_bytes)
            // Charge each retained interpreter's cap to the inactive cache.
            .saturating_add(
                self.visuals
                    .len()
                    .saturating_mul(vorkspace_core::visual::MEMORY_LIMIT_BYTES),
            )
    }
    pub(crate) fn suspend_history(&mut self, cx: &mut Context<Self>) -> bool {
        if self.visible
            || self.streaming
            || self.history_position.is_some()
            || !self.pending.is_empty()
            || self.saved_draft.is_some()
            || !self.note_pending.is_empty()
            || self.target_action.is_some()
        {
            return false;
        }
        let offset = self.list.logical_scroll_top();
        self.selection.clear();
        self.history_position = Some(HistoryPosition {
            key: self.rows.get(offset.item_ix).map(|row| row.key),
            offset,
            following: self.list.is_following_tail(),
        });
        self.source = Vec::new();
        self.source_bytes = 0;
        self.history_start = 0;
        self.history_loading = false;
        self.history_requested = false;
        self.history_search = None;
        self.prepared_parsers = HashMap::new();
        self.bot_groups = Default::default();
        self.attachment_images = HashMap::new();
        self.visuals = HashMap::new();
        self.messages = Vec::new();
        self.rows = Vec::new();
        self.reveals = HashMap::new();
        self.phase_layouts = HashMap::new();
        self.motions = HashMap::new();
        self.entrances = HashMap::new();
        self.taken = Vec::new();
        self.hover_rows = HashMap::new();
        self.hovered_turn = None;
        self.hovered_prompt = None;
        self.hovered_prompt_actions = None;
        self.action_anchors = HashMap::new();
        self.outline = Default::default();
        self.reflow = Default::default();
        self.follow = Default::default();
        // A running syntax task owns its input until completion; its old epoch
        // cannot repopulate this cache after suspension or after a quick reopen.
        self.highlight_epoch = self.highlight_epoch.wrapping_add(1);
        self.highlights = HashMap::new();
        self.highlighting = HashSet::new();
        self.highlight_queue = VecDeque::new();
        self.highlight_active = false;
        self.highlight_order = VecDeque::new();
        self.highlight_bytes = 0;
        self.list.reset(1);
        cx.notify();
        true
    }
    #[cfg(test)]
    pub(crate) fn restore_history(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        self.restore_prepared_history(PreparedHistory::new(rows, self.bot), cx);
    }
    pub(crate) fn restore_prepared_history(
        &mut self,
        history: PreparedHistory,
        cx: &mut Context<Self>,
    ) {
        let position = self.history_position.take();
        // Reopening also handles a source that became empty in another window.
        self.source = Vec::new();
        self.source_bytes = 0;
        self.messages = Vec::new();
        self.rows = Vec::new();
        self.history_start = 0;
        self.history_loading = false;
        self.history_requested = false;
        self.bot_groups.get_mut().clear();
        self.list.reset(1);
        self.install_prepared_history(history, cx);
        if let Some(position) = position {
            self.restore_reading_position(position);
        }
        cx.notify();
    }

    fn restore_reading_position(&mut self, position: HistoryPosition) {
        if position.following {
            self.list.set_follow_mode(FollowMode::Tail);
            self.list.scroll_to_end();
        } else {
            self.list.pause_following_tail();
            let item_ix = position
                .key
                .and_then(|key| self.rows.iter().position(|row| row.key == key))
                .unwrap_or(position.offset.item_ix.min(self.rows.len()));
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: position.offset.offset_in_item,
            });
        }
    }
}

#[cfg(test)]
mod tests;
