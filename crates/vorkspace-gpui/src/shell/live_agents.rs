//! A bounded projection of Shell's semantic session registry. The card owns its
//! presentation clock: ticking never hydrates history or rebuilds the session snapshot.
use super::*;
use crate::clock::Instant;
use gpui::{ListAlignment, ListOffset, ListState, list};
use std::time::Duration;
use vorkspace_core::project_appearance::ProjectAppearance;

const DEFAULT_ROWS: usize = 4;
const ROW_HEIGHT: f32 = 49.;
const ACTIVITY_ROW_HEIGHT: f32 = 67.;
const FRAME_TICK: Duration = Duration::from_millis(33);
const COMPACT_ROW_HEIGHT: f32 = 36.;
const ELAPSED_TICK: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Status {
    Approval,
    Working,
    Done,
}
impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Approval => "Need approval",
            Self::Working => "Working",
            Self::Done => "Done",
        }
    }
    pub(super) fn glyph(self) -> &'static str {
        match self {
            Self::Approval => "alert-circle",
            Self::Working => "response-activity",
            Self::Done => "check",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Agent {
    id: i64,
    title: String,
    project: String,
    cwd: String,
    appearance: ProjectAppearance,
    activity: Option<String>,
    provider: String,
    status: Status,
    selected: bool,
    updated_at: u64,
}
#[derive(Clone, Copy)]
struct Run {
    started: Instant,
    finished: Option<Instant>,
}

pub(super) struct LiveAgents {
    owner: gpui::WeakEntity<Shell>,
    agents: Vec<Agent>,
    runs: HashMap<i64, Run>,
    expanded: bool,
    compact: bool,
    visible: bool,
    scroll: ListState,
    window_visible: bool,
    visibility_subscription: Option<Subscription>,
    animation: Option<gpui::Task<()>>,
    animation_started: Instant,
    #[cfg(test)]
    frames: usize,
    list_focus: gpui::FocusHandle,
    bounds_subscription: Option<Subscription>,
    tick: Option<gpui::Task<()>>,
    #[cfg(test)]
    ticks: usize,
    #[cfg(test)]
    snapshots: usize,
}

impl LiveAgents {
    pub(super) fn new(owner: gpui::WeakEntity<Shell>, cx: &mut Context<Self>) -> Self {
        Self {
            owner,
            agents: Vec::new(),
            runs: HashMap::new(),
            expanded: false,
            compact: false,
            visible: false,
            scroll: ListState::new(0, ListAlignment::Top, px(0.)),
            window_visible: false,
            visibility_subscription: None,
            animation: None,
            animation_started: Instant::now(),
            #[cfg(test)]
            frames: 0,
            list_focus: cx.focus_handle(),
            bounds_subscription: None,
            tick: None,
            #[cfg(test)]
            ticks: 0,
            #[cfg(test)]
            snapshots: 0,
        }
    }

    /// SessionChanged records even a complete turn between two render frames,
    /// and keeps time while the card is hidden without keeping a timer alive.
    pub(super) fn record(&mut self, id: i64, working: bool, unseen: bool) {
        self.record_at(id, working, unseen, Instant::now());
    }

    fn record_at(&mut self, id: i64, working: bool, unseen: bool, now: Instant) {
        if working {
            let run = self.runs.entry(id).or_insert_with(|| Run {
                started: now,
                finished: None,
            });
            if run.finished.is_some() {
                *run = Run {
                    started: now,
                    finished: None,
                };
            }
        } else if unseen {
            if let Some(run) = self.runs.get_mut(&id) {
                run.finished.get_or_insert(now);
            }
        } else {
            self.runs.remove(&id);
        }
    }

    pub(super) fn hide(&mut self) {
        self.visible = false;
        self.tick = None;
        self.animation = None;
    }

    fn shown_count(&self) -> usize {
        if self.expanded {
            self.agents.len()
        } else {
            self.agents.len().min(DEFAULT_ROWS)
        }
    }

    pub(super) fn set_activity(&mut self, id: i64, label: Option<String>, cx: &mut Context<Self>) {
        if let Some(ix) = self.agents.iter().position(|agent| agent.id == id)
            && self.agents[ix].activity != label
        {
            self.agents[ix].activity = label;
            if ix < self.shown_count() {
                self.scroll.remeasure_items(ix..ix + 1);
            }
            cx.notify();
        }
    }

    // Status is always spoken by the row, even when no label is painted.
    fn activity_label<'a>(&self, agent: &'a Agent) -> Option<&'a str> {
        match agent.status {
            Status::Approval => Some(Status::Approval.label()),
            Status::Working => agent.activity.as_deref().filter(|label| {
                !label.trim().is_empty() && !matches!(label.trim(), "Working" | "Done")
            }),
            Status::Done => None,
        }
    }

    fn row_height(&self, agent: &Agent) -> f32 {
        if self.compact {
            COMPACT_ROW_HEIGHT
        } else if self.activity_label(agent).is_some() {
            ACTIVITY_ROW_HEIGHT
        } else {
            ROW_HEIGHT
        }
    }

    fn scroll_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        let page = self.scroll.viewport_bounds().size.height;
        let step = px(if self.compact {
            COMPACT_ROW_HEIGHT
        } else {
            ROW_HEIGHT
        });
        match event.keystroke.key.as_str() {
            "up" => self.scroll.scroll_by(-step),
            "down" => self.scroll.scroll_by(step),
            "pageup" => self.scroll.scroll_by(-page),
            "pagedown" => self.scroll.scroll_by(page),
            "home" => self.scroll.scroll_to(ListOffset::default()),
            "end" => self.scroll.scroll_to_end(),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Only inspect the viewport-sized presentation slice, never the registry or
    /// transcript. ListState owns the actual laid-out scroll anchor and clip.
    fn visible_has(&self, accepts: impl Fn(&Agent) -> bool) -> bool {
        if !self.visible || !self.window_visible || self.compact {
            return false;
        }
        let top = self.scroll.logical_scroll_top();
        let height = f32::from(self.scroll.viewport_bounds().size.height);
        let mut y = -f32::from(top.offset_in_item);
        for agent in self.agents[..self.shown_count()].iter().skip(top.item_ix) {
            if y >= height {
                break;
            }
            y += self.row_height(agent);
            if y > 0. && accepts(agent) {
                return true;
            }
        }
        false
    }

    fn working_count(&self) -> usize {
        self.agents
            .iter()
            .filter(|agent| agent.status == Status::Working)
            .count()
    }

    fn animation_visible(&self) -> bool {
        if self.compact {
            self.visible && self.window_visible && self.working_count() > 0
        } else {
            self.visible_has(|agent| agent.status == Status::Working)
        }
    }

    fn sync_animation(&mut self, cx: &mut Context<Self>) {
        if !self.animation_visible() {
            self.animation = None;
        } else if self.animation.is_none() {
            self.animation = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(FRAME_TICK).await;
                    let keep = this.update(cx, |this, cx| {
                        // Revalidate admission after every await. Hiding,
                        // scrolling, completion and disposal revoke the clock.
                        if !this.animation_visible() {
                            this.animation = None;
                            return false;
                        }
                        #[cfg(test)]
                        {
                            this.frames += 1;
                        }
                        cx.notify();
                        true
                    });
                    if !matches!(keep, Ok(true)) {
                        break;
                    }
                }
            }));
        }
    }

    fn frame_ms(&self) -> u64 {
        self.animation_started.elapsed().as_millis() as u64
    }

    fn toggle_expanded(&mut self, cx: &mut Context<Self>) {
        let top = self.scroll.logical_scroll_top();
        self.expanded = !self.expanded;
        // Expansion retains the reader's session and fractional offset, even
        // when a short collapsed viewport was deliberately scrolled. Collapsing
        // returns to the original four sessions.
        self.scroll.reset(self.shown_count());
        self.scroll.scroll_to(if self.expanded {
            top
        } else {
            ListOffset::default()
        });
        self.sync_timer(cx);
        cx.notify();
    }

    fn sync_timer(&mut self, cx: &mut Context<Self>) {
        self.sync_animation(cx);
        let ticking = self
            .visible_has(|agent| agent.status != Status::Done && self.runs.contains_key(&agent.id));
        if !ticking {
            self.tick = None;
        } else if self.tick.is_none() {
            self.tick = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(ELAPSED_TICK).await;
                    if this
                        .update(cx, |_this, cx| {
                            #[cfg(test)]
                            {
                                _this.ticks += 1;
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
    }

    fn publish(&mut self, mut agents: Vec<Agent>, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.snapshots += 1;
        }
        let now = Instant::now();
        for agent in &agents {
            self.record_at(
                agent.id,
                agent.status != Status::Done,
                agent.status == Status::Done,
                now,
            );
        }
        // Shell's event path also records hidden turns. Only discard identities
        // no longer present in the complete qualifying snapshot.
        let retained: HashSet<_> = agents.iter().map(|agent| agent.id).collect();
        self.runs.retain(|id, _| retained.contains(id));
        agents.sort_by_key(|agent| {
            (
                agent.status,
                self.runs.get(&agent.id).map(|run| run.started),
                agent.updated_at,
                agent.id,
            )
        });
        if self.agents != agents {
            // Preserve the top session and fractional offset. If it disappears,
            // use the closest surviving old neighbor, preferring the following
            // session on a tie, instead of jumping to the start of the list.
            let top = self.scroll.logical_scroll_top();
            let anchor = self
                .agents
                .iter()
                .enumerate()
                .filter(|(_, agent)| retained.contains(&agent.id))
                .min_by_key(|(ix, _)| (ix.abs_diff(top.item_ix), *ix < top.item_ix))
                .map(|(_, agent)| agent.id);
            if agents.is_empty() {
                self.expanded = false;
            }
            let next = anchor.and_then(|id| agents.iter().position(|agent| agent.id == id));
            self.agents = agents;
            self.scroll.reset(self.shown_count());
            if let Some(item_ix) = next.filter(|_| top.item_ix != 0 || top.offset_in_item != px(0.))
            {
                self.scroll.scroll_to(ListOffset {
                    item_ix: item_ix.min(self.shown_count().saturating_sub(1)),
                    ..top
                });
            }
            cx.notify();
        }
        self.sync_timer(cx);
    }

    fn present(&mut self, compact: bool, cx: &mut Context<Self>) -> bool {
        if self.agents.is_empty() || (compact && self.working_count() == 0) {
            self.hide();
            return false;
        }
        if self.compact != compact || !self.visible {
            self.compact = compact;
            self.scroll.remeasure();
            self.visible = true;
            cx.notify();
        }
        self.sync_timer(cx);
        true
    }

    fn elapsed(&self, agent: &Agent) -> Option<String> {
        self.runs.get(&agent.id).map(|run| {
            let elapsed = run
                .finished
                .unwrap_or_else(Instant::now)
                .saturating_duration_since(run.started);
            live_elapsed(elapsed)
        })
    }

    fn row(&self, agent: &Agent) -> gpui::Stateful<gpui::Div> {
        let id = agent.id;
        let owner = self.owner.clone();
        let elapsed = self.elapsed(agent);
        let activity = self.activity_label(agent);
        let label = [
            Some(agent.title.clone()),
            Some(agent.project.clone()),
            Some(agent.status.label().to_owned()),
            activity
                .filter(|label| *label != agent.status.label())
                .map(str::to_owned),
            elapsed.clone(),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
        let status_selector = format!("live-agent-{id}-{:?}", agent.status).to_lowercase();
        let row = crate::controls::button(("live-agent", id as usize), label, true)
            .debug_selector(move || format!("live-agent-{id}"))
            .flex_shrink_0()
            .aria_selected(agent.selected)
            .cursor(CursorStyle::Arrow)
            .rounded_md()
            .when(agent.selected, |d| d.bg(content(0.1)))
            .when(!agent.selected, |d| d.hover(|s| s.bg(content(0.08))))
            .on_click(move |_, window, cx| {
                let _ = owner.update(cx, |shell, cx| {
                    shell.selected_sessions.clear();
                    shell.selection_anchor = Some(id);
                    shell.open(id, window, cx);
                    shell.invalidate_chrome(cx);
                    cx.notify();
                });
            });
        let ink = match agent.status {
            Status::Approval => crate::theme::status(crate::theme::StatusColor::Warning, 1.),
            Status::Done => crate::theme::status(crate::theme::StatusColor::Success, 1.),
            Status::Working => content(0.5),
        };
        let status = div()
            .debug_selector(move || status_selector.clone())
            .size(px(12.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(if agent.status == Status::Working {
                crate::session_mark::spinner_dots(if self.compact { 400 } else { self.frame_ms() })
                    .into_any_element()
            } else {
                icon(agent.status.glyph(), 12.)
                    .text_color(ink)
                    .into_any_element()
            });
        if self.compact {
            return row
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .child(status);
        }
        row.w_full()
            .h(px(self.row_height(agent) - 1.))
            .px_2()
            .py_1p5()
            .flex()
            .flex_col()
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(crate::project_settings::project_icon(
                        &agent.appearance,
                        &agent.cwd,
                        8.,
                    ))
                    .child(
                        div()
                            .debug_selector(move || format!("live-agent-{id}-title"))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(content(1.))
                            .child(agent.title.clone()),
                    ),
            )
            .children(activity.map(|activity| {
                div()
                    .mt_1()
                    .pl_4()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_size(px(11.))
                    .line_height(px(14.))
                    .text_color(ink)
                    .child(
                        div()
                            .debug_selector(move || format!("live-agent-{id}-activity"))
                            .min_w_0()
                            .truncate()
                            .child(activity.to_owned()),
                    )
            }))
            .child(
                div()
                    .mt_1()
                    .pl_4()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_size(px(11.))
                    .line_height(px(14.))
                    .text_color(content(0.45))
                    .child(status)
                    .child(
                        div()
                            .debug_selector(move || format!("live-agent-{id}-provider"))
                            .size(px(12.))
                            .flex_shrink_0()
                            .child(provider_icon(&agent.provider, 12.)),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("live-agent-{id}-project"))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(agent.project.clone()),
                    )
                    .children(elapsed.map(|time| {
                        div()
                            .debug_selector(move || format!("live-agent-{id}-elapsed"))
                            .flex_shrink_0()
                            .child(time)
                    })),
            )
    }
}

// Same rounded, minimum-one-second and whole-unit formatting as the original.
fn live_elapsed(elapsed: Duration) -> String {
    let seconds = ((elapsed.as_millis().saturating_add(500) / 1000) as u64).max(1);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let (minutes, rest) = (seconds / 60, seconds % 60);
    if minutes < 60 {
        return if rest > 0 {
            format!("{minutes}m {rest}s")
        } else {
            format!("{minutes}m")
        };
    }
    let (hours, rest) = (minutes / 60, minutes % 60);
    if rest > 0 {
        format!("{hours}h {rest}m")
    } else {
        format!("{hours}h")
    }
}

impl Render for LiveAgents {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bounds_subscription.is_none() {
            self.bounds_subscription =
                Some(cx.observe_window_bounds(window, |_, _, cx| cx.notify()));
        }
        if self.visibility_subscription.is_none() {
            self.window_visible = window.is_visible();
            self.visibility_subscription = Some(cx.observe_window_visibility(
                window,
                |this, visibility, _, cx| {
                    this.window_visible = visibility.is_visible();
                    this.sync_timer(cx);
                },
            ));
        }
        if self.compact {
            // The compact surface has no list prepaint callback to admit motion.
            self.sync_timer(cx);
            let count = self.working_count();
            let owner = self.owner.clone();
            return div()
                .debug_selector(|| "sidebar-activity-card".into())
                .w_full()
                .px_2()
                .pb_2()
                .flex()
                .justify_center()
                .child(
                    crate::controls::button(
                        "compact-working",
                        format!(
                            "{count} working {}. Expand sidebar to choose a session",
                            if count == 1 { "session" } else { "sessions" }
                        ),
                        true,
                    )
                    .debug_selector(|| "compact-working".into())
                    .size(px(32.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(content(0.08)))
                    .child(crate::session_mark::spinner_dots(self.frame_ms()))
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |shell, cx| {
                            if !shell.workspace.project_rail_open {
                                shell.toggle_project_rail(cx);
                            }
                        });
                    }),
                )
                .into_any_element();
        }
        let count = self.agents.len();
        let extra = count.saturating_sub(DEFAULT_ROWS);
        let compact = self.compact;
        let expanded = self.expanded;
        let entity = cx.entity();
        let shown = self.shown_count();
        let height = px(self.agents[..shown]
            .iter()
            .scan(0., |height, agent| {
                if *height >= 360. {
                    return None;
                }
                *height += self.row_height(agent);
                Some(self.row_height(agent))
            })
            .sum::<f32>())
        .min(px(360.))
        .min((window.viewport_size().height * 0.36).max(px(48.)));
        let header = div()
            .debug_selector(|| "live-agents-working-header".into())
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px(px(14.))
            .py_1p5()
            .child(
                div()
                    .size(px(6.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(accent_color())
                    .shadow(vec![
                        BoxShadow::new(px(0.), px(0.), accent_color()).blur_radius(px(8.)),
                    ]),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child("Working"),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(count.to_string()),
            );
        let rows = div()
            .id("activity-rows")
            .debug_selector(|| "activity-rows".into())
            .role(Role::List)
            .aria_label(
                "Working sessions. Use arrow keys, Page Up, Page Down, Home or End to scroll.",
            )
            .track_focus(&self.list_focus)
            .tab_stop(true)
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.prevent_default()
            })
            .focus_visible(|s| s.bg(content(0.08)))
            .on_key_down(cx.listener(|this, event, _, cx| this.scroll_key(event, cx)))
            .when(!compact, |d| d.px_1())
            .when(extra == 0 && !compact, |d| d.pb_1())
            .h(height
                + if extra == 0 && !compact {
                    px(4.)
                } else {
                    px(0.)
                })
            .min_h_0()
            .flex_shrink_1()
            .child(
                list(self.scroll.clone(), move |ix, _, cx| {
                    entity.update(cx, |this, _| {
                        div()
                            .h(px(this.row_height(&this.agents[ix])))
                            .w_full()
                            .child(this.row(&this.agents[ix]))
                            .into_any_element()
                    })
                })
                .h_full()
                .w_full(),
            )
            // Runs after list prepaint: admission sees the real viewport and
            // clamped scroll anchor, including wheel, resize and keyboard moves.
            .child(
                gpui::canvas(
                    {
                        let entity = cx.entity();
                        move |_, _, cx| entity.update(cx, |this, cx| this.sync_timer(cx))
                    },
                    |_, _, _, _| {},
                )
                .size_0(),
            );
        let more = (extra > 0).then(|| {
            crate::controls::button(
                "live-agents-more",
                if expanded {
                    "Show fewer working sessions".to_owned()
                } else {
                    format!("Show {extra} more working sessions")
                },
                true,
            )
            .debug_selector(|| "live-agents-more".into())
            .flex_shrink_0()
            .group("live-agents-more")
            .aria_expanded(expanded)
            .cursor(CursorStyle::Arrow)
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .px_2()
            .py_1p5()
            .text_size(px(11.))
            .text_color(content(0.5))
            .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
            .child(
                icon(
                    if expanded {
                        "chevron-up"
                    } else {
                        "chevron-down"
                    },
                    12.,
                )
                .text_color(content(0.5))
                .group_hover("live-agents-more", |s| s.text_color(content(1.))),
            )
            .when(!compact, |d| {
                d.child(if expanded {
                    "Show less".to_owned()
                } else {
                    format!("{extra} more")
                })
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_expanded(cx)))
        });
        div()
            .id("live-agents")
            .debug_selector(|| "sidebar-activity-card".into())
            .role(Role::Group)
            .aria_label(format!("Working, {count} sessions"))
            // The rail owns the available height. Keep header/disclosure and
            // footer reachable while the list yields space in short windows.
            .flex()
            .flex_col()
            .flex_shrink_1()
            .min_h(px(if compact { 44. } else { 100. }) + if extra > 0 { px(26.) } else { px(0.) })
            .w_full()
            .px_2()
            .pb_2()
            .child(
                div()
                    .w_full()
                    .min_h_0()
                    .flex_shrink_1()
                    .rounded_lg()
                    .overflow_hidden()
                    .when(!compact, |d| d.bg(content(0.05)))
                    .flex()
                    .flex_col()
                    .when(!compact, |d| d.child(header))
                    .child(rows)
                    .children(more),
            )
            .into_any_element()
    }
}

impl Shell {
    /// Why a session belongs in the sidebar's activity: running, waiting on an
    /// approval, or finished unseen. Archived and deleting sessions never do.
    pub(super) fn sidebar_status(&self, id: i64, cx: &App) -> Option<Status> {
        let metadata = self.session_metadata(id, cx);
        if metadata.summary.archived || self.deleting.contains(&id) {
            return None;
        }
        if metadata.needs_approval() {
            Some(Status::Approval)
        } else if metadata.responding() || self.remote_marks.get(&id).is_some_and(|state| state.0) {
            Some(Status::Working)
        } else if !metadata.busy && self.unseen_finished.contains(&id) {
            Some(Status::Done)
        } else {
            None
        }
    }

    pub(super) fn sync_live_agents(&self, cx: &mut Context<Self>) {
        // App CLI and Bot launchers both install into this registry, independently
        // of tab/project selection and project folder collapse state. Remote marks
        // remain metadata-only; rendering this section never calls ensure_session.
        let ids: HashSet<_> = self
            .sessions
            .keys()
            .chain(self.remote_marks.keys())
            .chain(self.unseen_finished.iter())
            .copied()
            .collect();
        let agents: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                if !self.session_summaries.contains_key(&id) {
                    return None;
                }
                let status = self.sidebar_status(id, cx)?;
                let metadata = self.session_metadata(id, cx);
                Some(Agent {
                    id,
                    title: metadata.display_title(),
                    project: self.project_label(&metadata.summary.cwd),
                    cwd: metadata.summary.cwd.clone(),
                    appearance: self.appearance(&metadata.summary.cwd),
                    activity: self
                        .sessions
                        .get(&id)
                        .and_then(|view| view.read(cx).activity.clone()),
                    provider: metadata.summary.provider.clone(),
                    status,
                    selected: self.workspace.active == Some(id)
                        && !self.bot_active(cx)
                        && !self.app_view_open()
                        && self.app_search.is_none(),
                    updated_at: metadata.summary.updated_at,
                })
            })
            .collect();
        let hidden = self.classic_rail_width() <= 0.
            || self.settings.is_some()
            || !self.workspace.sidebar_activity_visible
            || !self.workspace.preferences.working_agents;
        self.live_agents.update(cx, |activity, cx| {
            activity.publish(agents, cx);
            // A fully hidden rail is unmounted, so its render callback cannot
            // be responsible for releasing the presentation timer.
            if hidden {
                activity.hide();
            }
        });
    }

    pub(super) fn live_agents_preview(
        &self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.workspace.sidebar_activity_visible || !self.workspace.preferences.working_agents {
            self.live_agents.update(cx, |activity, _| activity.hide());
            return None;
        }
        let visible = self
            .live_agents
            .update(cx, |activity, cx| activity.present(compact, cx));
        visible.then(|| self.live_agents.clone().into_any_element())
    }
}

#[cfg(test)]
#[path = "live_agents_tests.rs"]
mod tests;
