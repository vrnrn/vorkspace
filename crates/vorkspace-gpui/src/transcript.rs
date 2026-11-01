pub(crate) mod attachments;
pub(crate) mod bot_cards;
mod bot_rows;
#[cfg(test)]
mod bot_tests;
#[cfg(test)]
mod card_tests;
mod cards;
pub(crate) mod dates;
mod drafts;
mod follow;
#[cfg(test)]
mod follow_tests;
mod history;
mod pending_rows;
mod rebuilding;
mod response_bar;
mod row_bodies;
mod row_render;
mod streaming_frames;
pub(crate) use history::PreparedHistory;
mod links;
mod live_phase;
mod mascot;
mod measurements;
#[cfg(test)]
mod metrics_tests;
mod outline;
mod peers;
pub(crate) use peers::peer_looks;
pub(crate) mod phases;
pub(crate) mod remote_plans;
pub(crate) mod selection;
pub(crate) mod tasks;
mod tools;
mod visuals;

use crate::markdown::{self, Fence, MarkdownRow, Part};
use crate::popover::{Anchor, AnchorSlot, Side, popover};
use crate::response_actions::{self, Request as ResponseRequest};
use crate::syntax::{HighlightRequest, HighlightedDocument};
use gpui::{
    Animation, AnimationExt, AvailableSpace, Bounds, ClipboardItem, Context, FollowMode,
    ListAlignment, ListOffset, ListState, Pixels, Role, SharedString, Transformation, Window,
    canvas, div, list, percentage, prelude::*, px, rgb, size,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};
use vorkspace_core::tool::{ToolCall, ToolState, tasks::Panel};
use vorkspace_core::transcript::Row;
use vorkspace_model::markdown_model::MarkdownParser;
use vorkspace_ui::clock::Instant;
use vorkspace_ui::{
    reveal::{Disclosure, Motion, PacedText},
    theme::*,
};
/// Where a launched session stands, for its card in the chat.
#[derive(Clone, PartialEq)]
pub(crate) struct LaunchView {
    pub title: String,
    /// working, input, draft, failed, stopped or done.
    pub state: &'static str,
    pub harness: String,
    pub cwd: String,
}
/// A message the Bot has not taken yet, shown where it will land.
#[derive(Clone, PartialEq)]
pub(crate) struct PendingMessage {
    pub id: String,
    pub text: SharedString,
    pub error: Option<String>,
    pub sent_at: Option<u64>,
    pub attachments: Vec<std::path::PathBuf>,
    pub peer: Option<vorkspace_core::bot::peers::Origin>,
}
struct TakenMessage {
    text: SharedString,
    removed_at: Instant,
    entrance: Option<Instant>,
}
pub(crate) enum BotResponseAction {
    Quote {
        text: String,
        window: gpui::AnyWindowHandle,
    },
    /// Retry or discard a message that was not sent.
    Queue { id: String, remove: bool },
}
impl gpui::EventEmitter<BotResponseAction> for TranscriptView {}
pub(crate) struct StorageHistoryStale;
impl gpui::EventEmitter<StorageHistoryStale> for TranscriptView {}

fn work_title(model: Option<&str>, elapsed_ms: Option<u64>, live: bool) -> String {
    let model = model.map(str::trim).filter(|model| !model.is_empty());
    let verb = match (live, model.is_some()) {
        (true, true) => "working",
        (true, false) => "Working",
        (false, true) => "worked",
        (false, false) => "Worked",
    };
    let title = model.map_or_else(|| verb.to_owned(), |model| format!("{model} {verb}"));
    let Some(ms) = elapsed_ms else {
        return if live { format!("{title}…") } else { title };
    };
    let seconds = ms.saturating_add(500).saturating_div(1000).max(1);
    let duration = if seconds < 60 {
        format!("{seconds}s")
    } else if seconds % 60 == 0 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    };
    format!("{title} for {duration}")
}

struct Message {
    start: usize,
    display_start: usize,
    role: String,
    parser: MarkdownParser,
    under_line: bool,
    /// The provider run that owns this message's work receipt and phases.
    run: usize,
    /// The turn this message belongs to, as `Body::Actions` numbers it.
    turn: usize,
    attachments: Arc<[attachments::Attachment]>,
    peer: Option<vorkspace_core::bot::peers::Origin>,
}
#[derive(Clone)]
enum Body {
    RemotePlan(remote_plans::Plan),
    OlderHistory,
    SavedDraft,
    Markdown(MarkdownRow),
    Visual(Arc<vorkspace_core::visual::VisualDocument>),
    Plain(SharedString),
    Interjection(vorkspace_core::transcript::Interjection),
    BotCard(serde_json::Value),
    /// A queued message's bubble, by its index in `pending`.
    Pending(usize),
    /// LaunchedSessionCard for a session the run started.
    Launch(i64),
    /// SessionReview, the latest settled turn's recorded changes.
    Review {
        turn: usize,
    },
    Tool {
        call: Box<ToolCall>,
        group: Option<usize>,
        tail: bool,
    },
    Phase {
        title: String,
        group: usize,
        icon: &'static str,
        active: bool,
    },
    WorkLine {
        turn: usize,
        expandable: bool,
    },
    BotHeader {
        turn: usize,
    },
    Reasoning,
    Actions {
        turn: usize,
        complete: bool,
    },
}
#[derive(Clone)]
struct UiRow {
    key: (usize, usize),
    body: Body,
    message: usize,
    first: bool,
    last: bool,
    work: bool,
    work_tail: bool,
    /// Reasoning headers and their expanded Markdown share the tool phase.
    thought_phase: Option<(usize, bool)>,
}
impl UiRow {
    fn phase_group(&self) -> Option<usize> {
        self.thought_phase
            .map(|(group, _)| group)
            .or(match self.body {
                Body::Tool { group, .. } => group,
                _ => None,
            })
    }
    fn in_phase(&self) -> bool {
        self.phase_group().is_some()
    }
}
const TOP_INSET: f32 = 20.;
// Saved drafts are presentation rows, outside the provider transcript.
const SAVED_DRAFT_START: usize = usize::MAX - 1;
const BOTTOM_INSET: f32 = 40.;
const TOP_FADE: f32 = 24.;
/// `.turn-actions-hover`'s 120ms opacity transition, in seconds.
const HOVER_FADE: f32 = 0.12;

/// useBottomChatMotion's follow: the list keeps its real position at the end
/// while the painted chat eases into it, so a sent message or a streamed reply
/// glides up instead of jumping.
#[derive(Default)]
struct Follow {
    offset: f32,
    velocity: f32,
    growth: f32,
    /// Growth since the spring last stepped, the feed-forward's sample.
    grown: f32,
    measured: Option<FollowMeasure>,
    last_frame: Option<Instant>,
    #[cfg(test)]
    frame_elapsed: Option<f32>,
    last_step: Option<Instant>,
    settled_at: Option<Instant>,
    /// Growth keeps easing this long after a reply ends.
    until: Option<Instant>,
}
/// Units are px per 60fps frame, as in useBottomChatMotion.ts.
const SPRING_FRAME: f32 = 1000. / 60.;
/// Bot replies can appear together; keep their glide below 1080px/second.
const CHAT_FOLLOW_MAX_SPEED: f32 = 18.;
/// Steady frames only measure the tail. A large reveal measures its full
/// new distance once before handing over to this bounded window.
const FOLLOW_ROWS: usize = 80;

struct FollowMeasure {
    viewport: gpui::Size<Pixels>,
    bottom: Pixels,
    window_size: gpui::Size<Pixels>,
    rows: Vec<((usize, usize), f32)>,
    /// Regular chats fill the space below an anchored prompt before scrolling.
    footer: f32,
    /// A top-aligned chat that fits the viewport has no scroll distance yet.
    slack: f32,
}
impl FollowMeasure {
    fn resized_from(&self, previous: &Self) -> bool {
        self.viewport.width != previous.viewport.width || self.window_size != previous.window_size
    }
    /// Compare the suffix below a surviving row, like React's connected anchor.
    /// The composer's height moves that anchor too. Window resizing and changes
    /// to wrapping start a fresh baseline; internal vertical layout joins the glide.
    fn shift_from(&self, previous: &Self) -> f32 {
        if self.resized_from(previous) {
            return 0.;
        }
        let common = self.rows.iter().enumerate().find_map(|(next, (key, _))| {
            previous
                .rows
                .iter()
                .position(|(old, _)| old == key)
                .map(|old| (next, old))
        });
        let (next, old) = match common {
            Some(indices) => indices,
            None if self.rows.is_empty() || previous.rows.is_empty() => (0, 0),
            None => return 0.,
        };
        self.rows[next..]
            .iter()
            .map(|(_, height)| height)
            .sum::<f32>()
            - previous.rows[old..]
                .iter()
                .map(|(_, height)| height)
                .sum::<f32>()
            + self.footer
            - previous.footer
            + self.slack
            - previous.slack
            - f32::from(self.bottom - previous.bottom)
    }
}
impl Follow {
    /// stepFollowSpring: advance over `frames` and return the distance left.
    fn step(
        &mut self,
        mut offset: f32,
        grew: f32,
        mut frames: f32,
        sample_frames: f32,
        max_speed: f32,
    ) -> f32 {
        if grew < -1. {
            self.growth = 0.;
        } else {
            let observed = max_speed.min(grew.max(0.) / sample_frames.max(0.25));
            self.growth += 0.12 * (observed - self.growth);
        }
        let lead = (self.growth * 9.).min(32.);
        let mut velocity = self.velocity;
        while frames > 0. && offset > 0. {
            let h = frames.min(1.);
            frames -= h;
            let diff = (offset - lead).max(0.);
            velocity += h * ((0.7 * velocity + 0.05 * diff) / 1.25 - velocity);
            velocity = velocity.min(max_speed - self.growth);
            offset = (offset - (velocity + self.growth) * h).max(0.);
        }
        self.velocity = velocity;
        offset
    }
    fn add_shift(&mut self, shift: f32, viewport_height: f32, bot: bool) {
        let limit = (viewport_height * 0.75).max(0.);
        let offset = self.offset + shift;
        self.offset = if bot {
            offset.max(-limit)
        } else {
            offset.clamp(-limit, limit)
        };
        self.grown += shift;
    }
    fn reset(&mut self) {
        let measured = self.measured.take();
        let until = self.until;
        *self = Self::default();
        self.measured = measured;
        self.until = until;
    }
    fn settle(&mut self, now: Instant) {
        let (velocity, growth) = (self.velocity, self.growth);
        self.reset();
        self.velocity = velocity;
        self.growth = growth;
        self.last_frame = Some(now);
        self.last_step = Some(now);
        self.settled_at = Some(now);
    }
}

/// The first row of the prompt block that opens `turn`.
pub(crate) fn prompt_start(source: &[Row], turn: usize) -> usize {
    let mut at = turn.min(source.len());
    while at > 0 && source[at - 1].is_prompt() {
        at -= 1;
    }
    at
}
/// groupBotChatTurns: the turn a completion run at `turn` continues, if any.
pub(crate) fn bot_continues(source: &[Row], turn: usize) -> Option<usize> {
    let last = turn.checked_sub(1)?;
    if !source.get(last).is_some_and(|r| r.is_prompt()) {
        return None;
    }
    let prompt = prompt_start(source, turn);
    if prompt == 0
        || source[prompt..turn].iter().any(|r| r.role != "bot-system")
        || peers::prompt_origin(&source[prompt..turn]).is_some()
        || source[prompt - 1].is_prompt()
        || dates::bot_opens_stretch(source, prompt)
    {
        return None;
    }
    Some(
        source[..prompt]
            .iter()
            .rposition(|r| r.is_prompt())
            .map_or(0, |i| i + 1),
    )
}
/// isEmojiOnlyMessage: up to three emoji, which read as a reaction.
pub(crate) fn is_emoji_only(text: &str) -> bool {
    use unicode_segmentation::UnicodeSegmentation;
    let mut count = 0;
    for grapheme in text.trim().graphemes(true) {
        if grapheme.trim().is_empty() {
            continue;
        }
        let first = grapheme.chars().next().unwrap_or_default() as u32;
        let picture = matches!(first, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0x2300..=0x23FF | 0x3030 | 0x303D | 0x3297 | 0x3299 | 0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x21AA)
            || grapheme.contains('\u{20E3}');
        count += 1;
        if !picture || count > 3 {
            return false;
        }
    }
    count > 0
}

/// A single clipping boundary across virtualized siblings, matching React's
/// grid body. Fully revealed steps keep their height; only the boundary row clips.
struct PhaseLayout {
    width: Pixels,
    height: f32,
    rows: HashMap<(usize, usize), (f32, f32)>,
}
impl PhaseLayout {
    fn row_progress(&self, key: (usize, usize), progress: f32) -> f32 {
        let (top, height) = self.rows[&key];
        if height <= 0. {
            return 0.;
        }
        ((self.height * progress - top) / height).clamp(0., 1.)
    }
}

pub(crate) struct TranscriptView {
    selection: selection::Selection,
    selection_focus: Option<gpui::FocusHandle>,
    links: links::State,
    source: Vec<Row>,
    source_bytes: usize,
    attachment_images: HashMap<String, gpui::Entity<attachments::Photo>>,
    visuals: HashMap<usize, visuals::CachedVisual>,
    history_position: Option<history::HistoryPosition>,
    history_start: usize,
    history_loading: bool,
    history_requested: bool,
    history_search: Option<(usize, String)>,
    storage_history: Option<history::StorageHistory>,
    prepared_parsers: HashMap<usize, MarkdownParser>,
    bot: bool,
    bot_groups: std::cell::RefCell<HashMap<usize, usize>>,
    /// The Bot's look configuration and name, which sign its turns.
    bot_look: Option<(crate::avatar::BotLookConfig, SharedString)>,
    /// Roster looks by unique Bot name, for avatars on peer-message senders.
    peer_looks: HashMap<String, crate::avatar::BotLookConfig>,
    /// The turn whose actions show, and since when, as `.turn-actions-hover`.
    hovered_turn: Option<(usize, Instant)>,
    /// The prompt message whose copy action shows.
    hovered_prompt: Option<usize>,
    /// The prompt whose action strip the pointer is on. The strip hangs
    /// below the row's hitbox, so leaving the row must not hide it.
    hovered_prompt_actions: Option<usize>,
    /// Every hovered row and its turn. Entering the next row can arrive
    /// before leaving the last, so the turn is hovered while any row is.
    hover_rows: HashMap<(usize, usize), usize>,
    /// Messages entering the chat, by source start, as useBottomChatMotion.
    entrances: HashMap<usize, Instant>,
    /// The emoji a Bot answered a prompt with, by the prompt's start.
    reactions: HashMap<usize, (String, Option<Instant>)>,
    follow: Follow,
    /// botPendingTranscriptBlocks: queued messages, after the transcript.
    pending: Vec<PendingMessage>,
    saved_draft: Option<vorkspace_core::follow_up::Message>,
    draft_edit: Option<drafts::DraftEdit>,
    draft_editable: bool,
    draft_transfer: Option<bool>,
    /// Pending messages just taken, including any entrance still in flight.
    taken: Vec<TakenMessage>,
    pending_entrances: HashMap<String, Instant>,
    launches: HashMap<i64, LaunchView>,
    /// The session's latestTurnAccessory card.
    review: Option<gpui::AnyView>,
    /// The card has changes or an error to show, so it takes a row.
    review_shown: bool,
    /// A clock for the cards' spinners.
    started_at: Instant,
    /// One clock per frame for row motion. The list measures rows and the
    /// footer re-measures them; mixed times leave the spacer a few pixels
    /// short of a fast disclosure, so an anchored prompt bounces.
    frame_now: Instant,
    #[cfg(test)]
    fixed_animation_time: Option<Instant>,
    measurements: measurements::Measurements,
    messages: Vec<Message>,
    rows: Vec<UiRow>,
    streaming: bool,
    streaming_clock: Option<gpui::Task<()>>,
    chat_layout: bool,
    anchor_prompts: bool,
    smooth_reveal: bool,
    search_match: Option<(usize, String)>,
    highlights: HashMap<u64, Option<Arc<HighlightedDocument>>>,
    highlighting: HashSet<u64>,
    highlight_queue: VecDeque<Arc<Fence>>,
    highlight_active: bool,
    highlight_epoch: u64,
    highlight_order: VecDeque<(u64, usize)>,
    highlight_bytes: usize,
    copied: Option<u64>,
    copied_response: Option<(usize, Instant)>,
    note_pending: HashSet<usize>,
    note_feedback: HashMap<usize, Result<(), String>>,
    action_anchors: HashMap<(usize, &'static str), AnchorSlot>,
    hovered_action: Option<(usize, &'static str)>,
    pub(crate) target_action: Option<(usize, vorkspace_core::response::TransferKind)>,
    document: bool,
    side_question: bool,
    pub(crate) file_link_base: Option<String>,
    expanded: HashSet<usize>,
    agent_panels: tasks::AgentPanels,
    /// User messages opened past their four-line clamp, by start.
    expanded_prompts: HashSet<usize>,
    /// Collapsed user messages whose later chunks are hidden, by start.
    clamped_prompts: HashSet<usize>,
    phases: HashMap<usize, bool>,
    phase_toggle: Option<usize>,
    phase_layouts: HashMap<usize, PhaseLayout>,
    phase_scrolls: HashMap<usize, live_phase::LiveScroll>,
    windowed_phases: HashSet<usize>,
    phase_max_height: Pixels,
    work_folds: HashMap<usize, bool>,
    reveals: HashMap<usize, PacedText>,
    live_start: usize,
    motions: HashMap<(usize, usize), Motion>,
    reflow: vorkspace_ui::reflow_motion::ReflowMotion,
    frame_scheduled: bool,
    visible: bool,
    viewport: Bounds<Pixels>,
    anchor_turn: Option<usize>,
    prompt_rise: Option<(usize, Instant)>,
    prompt_rise_pending: bool,
    introduced_prompt: Option<usize>,
    show_jump: bool,
    jump_anchor: crate::popover::AnchorSlot,
    scroll_bound: bool,
    fold_targets: HashMap<usize, Option<usize>>,
    step_queue: HashMap<usize, Instant>,
    seen_steps: HashSet<usize>,
    pub(crate) turns: HashMap<usize, vorkspace_core::agent::TurnInfo>,
    #[cfg(test)]
    tooltip_text: std::cell::RefCell<Vec<gpui::TextLayout>>,
    provider: String,
    model: String,
    active_provider_turn_id: Option<String>,
    started: Option<vorkspace_ui::clock::Instant>,
    elapsed: HashMap<usize, std::time::Duration>,
    outline: outline::Outline,
    pub(crate) list: ListState,
    pub(crate) rendered: usize,
    pub(crate) render_passes: usize,
    #[cfg(test)]
    rebuild_passes: usize,
}
impl TranscriptView {
    /// Drive ordinary callbacks for projection-only tests with no window.
    /// GPUI advancement also runs queued work, so it must be outside App borrows.
    #[cfg(test)]
    fn settle_animation_frames(view: &gpui::Entity<Self>, cx: &mut gpui::TestAppContext) {
        for _ in 0..3 {
            cx.executor()
                .advance_clock(std::time::Duration::from_secs(1));
            view.update(cx, |view, cx| view.animate_frame(cx));
        }
    }

    fn animation_now(&self) -> Instant {
        #[cfg(test)]
        if let Some(now) = self.fixed_animation_time {
            return now;
        }
        Instant::now()
    }
    pub(crate) fn side_question(&mut self) {
        self.side_question = true;
    }
    /// AgentTranscript with `agentMascot`, `bottomAligned` and `inlineWork`.
    #[cfg(test)]
    pub(crate) fn bot_mode(&mut self, mascot: &str, color: u32, name: &str) {
        self.bot_mode_with_look(
            crate::avatar::BotLookConfig::from_legacy(mascot, color),
            name,
        );
    }
    pub(crate) fn bot_mode_with_look(&mut self, look: crate::avatar::BotLookConfig, name: &str) {
        self.bot_look = Some((look, name.to_owned().into()));
        if self.bot {
            return;
        }
        self.bot = true;
        self.anchor_prompts = false;
        self.chat_layout = true;
        // A short chat rests on the composer, like `justify-end`.
        self.list = ListState::new(self.rows.len() + 1, ListAlignment::Bottom, px(320.));
        self.list.set_follow_mode(FollowMode::Tail);
        self.scroll_bound = false;
    }
    pub(crate) fn reveal_search(&mut self, row: usize, query: &str, cx: &mut Context<Self>) {
        if row < self.history_start {
            self.history_search = Some((row, query.into()));
            self.list.pause_following_tail();
            self.load_older_history(cx);
            return;
        }
        let start = self
            .messages
            .iter()
            .rev()
            .find(|m| m.start <= row)
            .map(|m| m.start)
            .unwrap_or(row);
        let turn = self
            .messages
            .iter()
            .rev()
            .find(|m| m.start <= row)
            .map(|m| m.run)
            .unwrap_or(0);
        self.search_match = Some((start, query.to_owned()));
        self.expanded.insert(start);
        self.expanded_prompts.insert(start);
        self.work_folds.insert(turn, true);
        for message in &self.messages {
            if message.run == turn {
                self.phases.insert(message.start, true);
            }
        }
        self.list.pause_following_tail();
        self.rebuild(0, cx);
        let candidates = || {
            self.rows.iter().enumerate().filter(|(_, r)| {
                r.key.0 == start
                    && !matches!(r.body, Body::WorkLine { .. } | Body::BotHeader { .. })
            })
        };
        let matching = candidates()
            .find(|(_, r)| {
                let text = match &r.body {
                    Body::Markdown(md) => markdown::row_text(md),
                    Body::Plain(text) => text.to_string(),
                    Body::Interjection(note) => note.text.clone(),
                    Body::Tool { .. } => query.to_owned(),
                    _ => String::new(),
                };
                !vorkspace_core::search::literal_ranges(&text, query).is_empty()
            })
            .map(|(ix, _)| ix);
        if let Some(item_ix) = matching.or_else(|| candidates().next().map(|(ix, _)| ix)) {
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: px(0.),
            });
        }
        cx.notify();
    }
    pub(crate) fn source_rows(&self) -> &[Row] {
        &self.source
    }
    pub(crate) fn document(text: String, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new();
        view.document = true;
        view.patch(
            0,
            vec![Row {
                role: "assistant".into(),
                text,
                first: true,
                sent_at: None,
            }],
            cx,
        );
        view
    }

    pub(crate) fn active_turn(&self) -> Option<usize> {
        let start = self.live_start.min(self.source.len());
        let prompt = self.source[start..]
            .iter()
            .position(|row| row.first && row.is_prompt())?
            + start;
        Some(
            (prompt + 1..self.source.len())
                .find(|&at| self.source[at].first)
                .unwrap_or(self.source.len()),
        )
    }
    pub(crate) fn metrics(&mut self, metrics: vorkspace_core::agent::TurnMetrics) {
        let Some(turn) = self.active_turn() else {
            return;
        };
        self.turns.entry(turn).or_default().metrics = Some(metrics);
    }
    pub(crate) fn record_provider_turn(&mut self, id: String) {
        self.active_provider_turn_id = Some(id.clone());
        let Some(turn) = self.active_turn() else {
            return;
        };
        self.turns.entry(turn).or_default().provider_turn_id = Some(id);
    }
    pub(crate) fn response_rows(&self, turn: usize) -> (Vec<Row>, Vec<Row>) {
        let (rows, through) = self.response_slices(turn);
        (rows.to_vec(), through.to_vec())
    }

    // Rendering needs only a borrowed response. Own history only when an
    // explicit transfer leaves this view, never for each action-strip repaint.
    fn response_slices(&self, turn: usize) -> (&[Row], &[Row]) {
        let end = (turn..self.source.len())
            .find(|&i| self.source[i].is_prompt())
            .unwrap_or(self.source.len());
        let mut start = self.source[..turn.min(self.source.len())]
            .iter()
            .rposition(|r| r.is_prompt())
            .unwrap_or(turn);
        while start > 0 && !self.source[start].first {
            start -= 1;
        }
        (&self.source[start..end], &self.source[..end])
    }
    pub(crate) fn note_result(
        &mut self,
        turn: usize,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        self.note_pending.remove(&turn);
        let success = result.is_ok();
        self.note_feedback.insert(turn, result);
        if success && self.selection_turn() == Some(turn) {
            self.selection.clear();
        }
        cx.notify();
        if success {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.note_feedback.remove(&turn);
                    cx.notify();
                });
            })
            .detach();
        }
    }
    fn footer_ix(&self) -> usize {
        self.rows.len()
    }
    /// The first row of the prompt block that opens `turn`.
    fn prompt_start(&self, turn: usize) -> usize {
        prompt_start(&self.source, turn)
    }
    fn chat_animating(&self) -> bool {
        !self.entrances.is_empty()
            || !self.pending_entrances.is_empty()
            || (self.launches.values().any(|l| l.state == "working")
                && self.rows.iter().rev().take(200).any(|r| {
                    matches!(r.body, Body::Launch(_))
                        || matches!(&r.body, Body::BotCard(v) if v["type"] == "session")
                }))
            || self.follow.offset.abs() >= 0.25
            || self.reactions.values().any(|(_, born)| born.is_some())
            || self
                .hovered_turn
                .is_some_and(|(_, since)| since.elapsed().as_secs_f32() < HOVER_FADE)
    }
    pub(crate) fn new() -> Self {
        Self {
            selection: Default::default(),
            selection_focus: None,
            links: Default::default(),
            source: Vec::new(),
            source_bytes: 0,
            attachment_images: HashMap::new(),
            visuals: HashMap::new(),
            history_position: None,
            history_start: 0,
            history_loading: false,
            history_requested: false,
            history_search: None,
            storage_history: None,
            prepared_parsers: HashMap::new(),
            bot: false,
            bot_groups: Default::default(),
            bot_look: None,
            peer_looks: HashMap::new(),
            hovered_turn: None,
            hovered_prompt: None,
            hovered_prompt_actions: None,
            hover_rows: HashMap::new(),
            entrances: HashMap::new(),
            reactions: HashMap::new(),
            follow: Follow::default(),
            pending: Vec::new(),
            saved_draft: None,
            draft_edit: None,
            draft_editable: true,
            draft_transfer: None,
            taken: Vec::new(),
            pending_entrances: HashMap::new(),
            launches: HashMap::new(),
            review: None,
            review_shown: false,
            started_at: Instant::now(),
            frame_now: Instant::now(),
            #[cfg(test)]
            fixed_animation_time: None,
            measurements: Default::default(),
            messages: Vec::new(),
            rows: Vec::new(),
            streaming: false,
            streaming_clock: None,
            chat_layout: true,
            anchor_prompts: true,
            smooth_reveal: true,
            search_match: None,
            highlights: HashMap::new(),
            highlighting: HashSet::new(),
            highlight_queue: VecDeque::new(),
            highlight_active: false,
            highlight_epoch: 0,
            highlight_order: VecDeque::new(),
            highlight_bytes: 0,
            copied: None,
            copied_response: None,
            note_pending: HashSet::new(),
            note_feedback: HashMap::new(),
            action_anchors: HashMap::new(),
            hovered_action: None,
            target_action: None,
            document: false,
            side_question: false,
            file_link_base: None,
            expanded: HashSet::new(),
            agent_panels: Default::default(),
            expanded_prompts: HashSet::new(),
            clamped_prompts: HashSet::new(),
            phases: HashMap::new(),
            phase_toggle: None,
            phase_layouts: HashMap::new(),
            phase_scrolls: HashMap::new(),
            windowed_phases: HashSet::new(),
            phase_max_height: px(280.),
            work_folds: HashMap::new(),
            reveals: HashMap::new(),
            live_start: 0,
            motions: HashMap::new(),
            reflow: Default::default(),
            frame_scheduled: false,
            visible: true,
            viewport: Bounds::default(),
            anchor_turn: None,
            prompt_rise: None,
            prompt_rise_pending: false,
            introduced_prompt: None,
            show_jump: false,
            jump_anchor: Default::default(),
            scroll_bound: false,
            fold_targets: HashMap::new(),
            step_queue: HashMap::new(),
            seen_steps: HashSet::new(),
            turns: HashMap::new(),
            #[cfg(test)]
            tooltip_text: Default::default(),
            provider: "codex".into(),
            model: "Codex".into(),
            active_provider_turn_id: None,
            started: None,
            elapsed: HashMap::new(),
            outline: outline::Outline::default(),
            list: {
                // The trailing item holds the viewport's bottom padding and,
                // after a send, the unused part of the current turn's min-height.
                let list = ListState::new(1, ListAlignment::Top, px(320.));
                list.set_follow_mode(FollowMode::Tail);
                list
            },
            rendered: 0,
            render_passes: 0,
            #[cfg(test)]
            rebuild_passes: 0,
        }
    }
    pub(crate) fn apply_preferences(
        &mut self,
        p: &vorkspace_ui::preferences::Preferences,
        cx: &mut Context<Self>,
    ) {
        let chat = p.transcript_layout == "chat" || self.bot;
        let anchor_prompts = p.anchor_prompts && !self.bot;
        if self.chat_layout == chat
            && self.anchor_prompts == anchor_prompts
            && self.smooth_reveal == p.paced_reveal
        {
            return;
        }
        self.chat_layout = chat;
        self.anchor_prompts = anchor_prompts;
        self.smooth_reveal = p.paced_reveal;
        if !self.anchor_prompts {
            self.anchor_turn = None;
        }
        if !self.smooth_reveal {
            for reveal in self.reveals.values_mut() {
                reveal.settle(Instant::now());
            }
        }
        self.rebuild(0, cx);
        cx.notify();
    }
    pub(crate) fn visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.selection.hide();
        let now = Instant::now();
        // Settled tabs retain their parsed rows, measured heights and scroll
        // anchor. Only undisplayed text or outgoing animation rows need a
        // source rebuild when visibility changes.
        let rebuild_from = self
            .reveals
            .iter()
            .filter(|(_, reveal)| reveal.behind())
            .map(|(&at, _)| at)
            .chain(
                self.motions
                    .iter()
                    .filter(|(_, motion)| !motion.opening)
                    .map(|(key, _)| key.0),
            )
            .min();
        let panels: HashSet<_> = self.agent_panels.animate().into_iter().collect();
        if !self.motions.is_empty() || !panels.is_empty() {
            self.phase_layouts.clear();
            for (ix, row) in self.rows.iter().enumerate() {
                if self.motions.contains_key(&row.key) || panels.contains(&row.key.0) {
                    self.list.remeasure_items(ix..ix + 1);
                }
            }
        }
        self.prompt_rise = None;
        self.prompt_rise_pending = false;
        self.entrances.clear();
        self.follow = Follow::default();
        for reveal in self.reveals.values_mut() {
            reveal.settle(now);
        }
        self.visible = false;
        for scroll in self.phase_scrolls.values_mut() {
            scroll.enable(false);
        }
        self.motions.clear();
        self.agent_panels.settle();
        if let Some(start) = rebuild_from.filter(|_| !self.source.is_empty()) {
            self.rebuild(start.min(self.source.len() - 1), cx);
        }
        self.visible = visible;
        self.sync_streaming_clock(cx);
        if visible {
            cx.notify();
        }
    }
    pub(crate) fn identity(&mut self, provider: &str, model: &str) {
        self.provider = provider.into();
        self.model = model.into();
        if self.streaming
            && let Some(turn) = self.active_turn()
        {
            let info = self.turns.entry(turn).or_default();
            info.provider = provider.into();
            info.model = model.into();
        }
    }
    /// Latest assistant message, including its virtualized continuation rows.
    pub(crate) fn notification_reply_body(&self) -> String {
        let Some(end) = self
            .source
            .iter()
            .rposition(|row| row.role == "assistant" && !row.text.trim().is_empty())
        else {
            return String::new();
        };
        let mut start = end;
        while start > 0 && !self.source[start].first && self.source[start - 1].role == "assistant" {
            start -= 1;
        }
        let mut text = String::new();
        for row in &self.source[start..=end] {
            text.push_str(&row.text);
            // A notification needs only the first paragraph and 240 code units.
            // Retain enough extra text to distinguish a 240-unit body from one
            // requiring the ellipsis, without cloning a giant reply.
            if text.encode_utf16().count() > 480 {
                break;
            }
        }
        crate::notifications::delivery::clip_body(&text)
    }

    /// Tauri's appendUser: paint the prompt and working receipt in the send
    /// handler. The worker replaces these same rows after saving them. Build
    /// the attachment manifest without touching the filesystem on the UI thread.
    pub(crate) fn submit_prompt(
        &mut self,
        prompt: &str,
        settings: &vorkspace_core::agent::Settings,
        cx: &mut Context<Self>,
    ) {
        let mut submitted = vorkspace_core::transcript::Transcript::default();
        submitted.begin(if settings.internal_message {
            "bot-system"
        } else {
            "user"
        });
        submitted.rows[0].sent_at = settings
            .bot_sent_at
            .or_else(|| {
                settings
                    .turn_model
                    .as_ref()
                    .and_then(|turn| turn.started_at)
            })
            .or(submitted.rows[0].sent_at);
        submitted.append(prompt);
        if !settings.attachments.is_empty() {
            submitted.rows.push(Row {
                role: vorkspace_core::transcript::attachments::ROLE.into(),
                text: vorkspace_core::transcript::attachments::from_paths(&settings.attachments)
                    .to_string(),
                first: true,
                sent_at: None,
            });
        }
        submitted.begin("assistant");
        self.patch(self.source.len(), submitted.rows, cx);
    }

    fn toggle(&mut self, start: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(&start) {
            self.expanded.insert(start);
        }
        self.list.pause_following_tail();
        self.rebuild(start, cx);
    }
    /// "Show more" opens a message in place, without a transition.
    fn toggle_prompt(&mut self, start: usize, cx: &mut Context<Self>) {
        if !self.expanded_prompts.remove(&start) {
            self.expanded_prompts.insert(start);
        }
        self.list.pause_following_tail();
        self.rebuild(start, cx);
    }
    /// Wrapped lines of a user message's text at the bubble's text width.
    fn prompt_lines(&self, text: &str, window: &Window) -> usize {
        let column = self.viewport.size.width.min(px(COLUMN));
        // Row padding (`pl-14 pr-4` in the chat layout, `px-4` otherwise),
        // the bubble's 36rem cap and its `px-3`.
        let bubble = if self.chat_layout {
            (column - px(72.)).min(px(576.))
        } else {
            column - px(32.)
        };
        let text = SharedString::from(text.trim_end().to_owned());
        let run = gpui::TextRun {
            len: text.len(),
            font: window.text_style().font(),
            color: Default::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_text(text, px(14.), &[run], Some(bubble - px(24.)), None)
            .map_or(0, |lines| {
                lines
                    .iter()
                    .map(|line| line.wrap_boundaries.len() + 1)
                    .sum()
            })
    }
    fn toggle_phase(&mut self, group: usize, open: bool, cx: &mut Context<Self>) {
        self.phase_toggle = Some(group);
        self.phases.insert(group, open);
        self.list.pause_following_tail();
        self.rebuild(group, cx);
    }
}
impl gpui::EventEmitter<ResponseRequest> for TranscriptView {}
impl Render for TranscriptView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.history_requested)
            && !self.list.is_following_tail()
            && self.list.logical_scroll_top().item_ix <= 2
        {
            self.load_older_history(cx);
        }
        if self.selection_focus.is_none() {
            self.selection_focus = Some(cx.focus_handle());
        }
        self.selection.begin_frame();
        self.phase_max_height = live_phase::max_height(window);
        self.frame_now = self.animation_now();
        self.begin_measurements(window);
        if !self.scroll_bound {
            self.scroll_bound = true;
            self.list
                // The list holds its state borrow while delivering the event.
                // Read the final scroll geometry in our trailing layout probe.
                .set_scroll_handler(cx.listener(|this, _, _, cx| {
                    this.selection.clear();
                    this.history_requested = this.history_start > 0 || this.storage_has_older();
                    cx.notify()
                }));
        }
        self.update_follow(window, cx);
        let now = self.frame_now;
        if self.prompt_rise_pending {
            self.prompt_rise_pending = false;
            if let Some((_, born)) = self.prompt_rise.as_mut() {
                *born = now;
            }
        }
        self.rendered = 0;
        self.render_passes += 1;
        let view = cx.entity().downgrade();
        let after = cx.entity().downgrade();
        div()
            .id("transcript")
            .key_context("TranscriptSelection")
            .track_focus(self.selection_focus.as_ref().unwrap())
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::open_link_menu))
            .capture_any_mouse_down(cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                if event.button == gpui::MouseButton::Right && this.links.open.is_none() {
                    window.focus(this.selection_focus.as_ref().unwrap(), cx);
                }
            }))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.selection.clear_if_not_dragging();
                cx.notify();
            }))
            .on_key_down(cx.listener(|this,event:&gpui::KeyDownEvent,_,cx|{if event.keystroke.key=="escape" && this.selection.active(){this.selection.clear();cx.notify();cx.stop_propagation()}}))
            .on_mouse_move(cx.listener(Self::move_selection))
            .on_mouse_up(gpui::MouseButton::Left,cx.listener(Self::finish_selection))
            .on_mouse_up_out(gpui::MouseButton::Left,cx.listener(Self::finish_selection))
            .role(Role::Log)
            .aria_label(if self.document{"Note body"}else{"Conversation transcript"})
            .relative()
            .size_full()
            .overflow_hidden()
            .child(canvas(move |bounds, window, cx| {
                let _ = view.update(cx, |this, cx| {
                    if this.viewport != bounds {
                        let reflow = this.viewport.size.width != bounds.size.width;
                        // Byte anchors remain valid when the same text rewraps;
                        // only the floating menu and drag geometry are stale.
                        this.selection.hide_menu();
                        this.viewport = bounds;
                        if reflow {
                            this.restore_reading_glyph(window, cx);
                        }
                        this.list.remeasure_items(this.footer_ix()..this.footer_ix() + 1);
                    }
                    let resized = this.update_follow_viewport(bounds, window);
                    // Refresh the row-height baseline after a real resize even
                    // when the chat is idle, ready for the next send.
                    this.schedule_frame(resized, window, cx);
                });
            }, |bounds, _, window, _| paint_edge_fade(bounds, true, window)).absolute().top_0().left_0().size_full())
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix: usize, window, cx| {
                        if ix == this.footer_ix() { this.footer(window, cx) }
                        else {
                            let row = this.row(ix, window, cx);
                            this.reflow.wrap(this.rows[ix].key, row, this.follow.offset)
                        }
                    }),
                )
                .pt(px(TOP_INSET))
                .size_full(),
            )
            .child(canvas(move |_, _, cx| {
                let _ = after.update(cx, |this, cx| {
                    this.selection.remember_reader(this.viewport, &this.list);
                    this.update_jump(cx);
                    this.update_outline(cx);
                });
            }, |bounds, _, window, _| paint_edge_fade(bounds, false, window)).absolute().top_0().left_0().size_full())
            .children(self.outline(window, cx))
            .children(self.selection_menu(cx))
            .child(self.link_overlays(cx))
            .when(self.show_jump, |d| {
                d.child(div().absolute().bottom(px(8.)).left_0().w_full().flex().justify_center().child(
                    div().id("jump-to-bottom").child(self.jump_anchor.probe()).role(Role::Button).aria_label("Scroll to bottom")
                        .relative().flex().items_center().gap(px(6.)).h(px(30.)).px(px(10.))
                        .rounded_full().border_1().border_color(content(0.1))
                        .text_size(px(12.)).line_height(px(16.)).text_color(content(0.8))
                        .hover(|s| s.text_color(content(1.)).bg(content(0.04)))
                        .child(canvas(|_, _, _| {}, |bounds, _, window, _| {
                            window.paint_quad(gpui::fill(bounds, content(0.04)).corner_radii(px(15.)).border_color(
                                gpui_apple::metal_renderer::backdrop_blur_marker(12. * window.scale_factor())));
                        }).absolute().top_0().left_0().size_full())
                        .child(icon("arrow-down", 14.).text_color(content(0.5)))
                        .child("Scroll to bottom")
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, _, cx| this.jump_to_bottom(cx)))
                ))
            })
    }
}

fn paint_edge_fade(bounds: Bounds<Pixels>, begin: bool, window: &mut Window) {
    window.paint_quad(gpui::fill(bounds, gpui::black()).border_color(
        gpui_apple::metal_renderer::edge_fade_marker(
            begin,
            TOP_FADE * window.scale_factor(),
            BOTTOM_INSET * window.scale_factor(),
        ),
    ));
}

#[cfg(test)]
mod scroll_tests;

#[cfg(test)]
mod response_render_tests {
    use super::*;
    fn row(role: &str, text: &str, first: bool) -> Row {
        Row {
            role: role.into(),
            text: text.into(),
            first,
            sent_at: None,
        }
    }

    #[test]
    fn borrowed_responses_match_owned_transfers_through_mutations_and_continuations() {
        let mut view = TranscriptView::new();
        view.source = vec![
            row("user", "prompt", true),
            row("user", "prompt fragment", false),
            row("assistant", "answer", true),
            row("tool", "result", true),
            row("bot-system", "continue", true),
            row("assistant", "continued", true),
            row("user", "next", true),
            row("assistant", "latest", true),
        ];
        for replacement in ["latest", "streamed replacement", ""] {
            view.source[7].text = replacement.into();
            for (turn, start, end) in [(2, 0, 4), (5, 4, 6), (7, 6, 8)] {
                let (rows, through) = view.response_slices(turn);
                assert_eq!(rows, &view.source[start..end]);
                assert_eq!(through, &view.source[..end]);
                assert!(std::ptr::eq(rows.as_ptr(), view.source[start..].as_ptr()));
                let (owned, history) = view.response_rows(turn);
                assert_eq!(rows, owned);
                assert_eq!(through, history);
                assert_eq!(
                    vorkspace_core::response::prose(rows),
                    vorkspace_core::response::prose(&owned)
                );
            }
        }
    }

    #[test]
    #[ignore = "explicit response construction CPU benchmark"]
    fn response_render_benchmark() {
        use std::{hint::black_box, time::Instant};
        let mut view = TranscriptView::new();
        view.source = (0..256)
            .map(|_| row("tool", &"x".repeat(128 * 1024), true))
            .collect();
        view.source.push(row("user", "prompt", true));
        view.source.push(row("assistant", "answer", true));
        for _ in 0..5 {
            let start = Instant::now();
            for _ in 0..30 {
                let (rows, _) = view.response_rows(257);
                black_box(vorkspace_core::response::prose(&rows));
            }
            let baseline = start.elapsed();
            let start = Instant::now();
            for _ in 0..30 {
                let (rows, _) = view.response_slices(257);
                black_box(vorkspace_core::response::prose(rows));
            }
            eprintln!(
                "response history_bytes={} samples=30 baseline_us={} borrowed_us={}",
                256 * 128 * 1024,
                baseline.as_micros(),
                start.elapsed().as_micros()
            );
        }
    }
}
