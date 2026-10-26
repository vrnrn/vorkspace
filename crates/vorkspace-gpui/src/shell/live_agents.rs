//! A bounded projection of Shell's semantic session registry. The card owns its
//! presentation clock: ticking never hydrates history or rebuilds the session snapshot.
use super::*;
use crate::clock::Instant;
use std::time::Duration;

const PAGE_SIZE: usize = 4;
const ELAPSED_TICK: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    Approval,
    Working,
    Done,
}
impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Approval => "Needs approval",
            Self::Working => "Working",
            Self::Done => "Done",
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Self::Approval => "alert-circle",
            Self::Working => "response-activity",
            Self::Done => "check-circle-2",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Agent {
    id: i64,
    title: String,
    project: String,
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
    page: usize,
    compact: bool,
    visible: bool,
    scroll: ScrollHandle,
    tick: Option<gpui::Task<()>>,
    #[cfg(test)]
    ticks: usize,
    #[cfg(test)]
    snapshots: usize,
}

impl LiveAgents {
    pub(super) fn new(owner: gpui::WeakEntity<Shell>) -> Self {
        Self {
            owner,
            agents: Vec::new(),
            runs: HashMap::new(),
            page: 0,
            compact: false,
            visible: false,
            scroll: ScrollHandle::new(),
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
    }

    fn page_agents(&self) -> &[Agent] {
        let start = (self.page * PAGE_SIZE).min(self.agents.len());
        &self.agents[start..(start + PAGE_SIZE).min(self.agents.len())]
    }

    fn sync_timer(&mut self, cx: &mut Context<Self>) {
        let ticking = self.visible
            && !self.compact
            && self
                .page_agents()
                .iter()
                .any(|agent| agent.status != Status::Done && self.runs.contains_key(&agent.id));
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
            // Keep the current page through title, group and selection updates.
            self.page = self.page.min(agents.len().saturating_sub(1) / PAGE_SIZE);
            self.agents = agents;
            cx.notify();
        }
        self.sync_timer(cx);
    }

    fn present(&mut self, compact: bool, cx: &mut Context<Self>) -> bool {
        if self.agents.is_empty() {
            self.hide();
            return false;
        }
        if self.compact != compact || !self.visible {
            self.compact = compact;
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
                .saturating_duration_since(run.started)
                .as_secs();
            if elapsed < 60 {
                format!("{elapsed}s")
            } else if elapsed < 3600 {
                format!("{}m {}s", elapsed / 60, elapsed % 60)
            } else {
                format!("{}h {}m", elapsed / 3600, elapsed / 60 % 60)
            }
        })
    }

    fn row(&self, agent: &Agent) -> AnyElement {
        let id = agent.id;
        let owner = self.owner.clone();
        let elapsed = self.elapsed(agent);
        let label = format!(
            "{}, {}, {}{}",
            agent.title,
            agent.project,
            agent.status.label(),
            elapsed
                .as_ref()
                .map(|time| format!(", {time}"))
                .unwrap_or_default()
        );
        let status_selector = format!("live-agent-{id}-{:?}", agent.status).to_lowercase();
        let row = div()
            .id(("live-agent", id as usize))
            .debug_selector(move || format!("live-agent-{id}"))
            .role(Role::Button)
            .flex_shrink_0()
            .aria_label(label)
            .aria_selected(agent.selected)
            .focusable()
            .tab_stop(true)
            .cursor(CursorStyle::Arrow)
            .rounded_md()
            .when(agent.selected, |d| d.bg(content(0.1)))
            .hover(|s| s.bg(content(0.08)))
            .focus(|s| s.bg(content(0.1)))
            .on_click(move |_, window, cx| {
                let _ = owner.update(cx, |shell, cx| {
                    shell.open(id, window, cx);
                    shell.invalidate_chrome(cx);
                });
            });
        let status = icon(agent.status.glyph(), 12.)
            .debug_selector(move || status_selector.clone())
            .text_color(if agent.status == Status::Working {
                content(0.7)
            } else {
                accent_color()
            });
        if self.compact {
            return row
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .child(status)
                .into_any_element();
        }
        row.w_full()
            .px_2()
            .py_1p5()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(provider_icon(&agent.provider, 12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(content(1.))
                            .child(agent.title.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(content(0.7))
                    .child(status)
                    .child(agent.status.label())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(agent.project.clone()),
                    )
                    .children(elapsed.map(|time| div().flex_shrink_0().child(time))),
            )
            .into_any_element()
    }
}

impl Render for LiveAgents {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.agents.len();
        let compact = self.compact;
        let pages = count.div_ceil(PAGE_SIZE);
        div()
            .id("live-agents")
            .debug_selector(|| "sidebar-activity-card".into())
            .role(Role::Group)
            .aria_label(format!("Activity, {count} sessions"))
            .flex_shrink_0()
            .w_full()
            .px_2()
            .pb_2()
            .child(
                div()
                    .w_full()
                    .rounded_lg()
                    .overflow_hidden()
                    .when(!compact, |d| d.bg(content(0.03)).p_1())
                    .when(!compact, |d| {
                        d.child(
                            section_label("Activity")
                                .child(div().text_size(px(11.)).child(count.to_string())),
                        )
                    })
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id("activity-rows")
                            .debug_selector(|| "activity-rows".into())
                            .max_h((window.viewport_size().height * 0.28).max(px(48.)))
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(self.page_agents().iter().map(|agent| self.row(agent))),
                    )
                    .when(pages > 1, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .child(
                                    icon_button_enabled(
                                        "activity-previous",
                                        "Previous active sessions",
                                        "chevron-left",
                                        self.page > 0,
                                    )
                                    .debug_selector(|| "activity-previous".into())
                                    .focusable()
                                    .tab_stop(true)
                                    .when(self.page > 0, |d| {
                                        d.on_click(cx.listener(|this, _, _, cx| {
                                            this.page -= 1;
                                            this.scroll.set_offset(Point::default());
                                            this.sync_timer(cx);
                                            cx.notify();
                                        }))
                                    }),
                                )
                                .when(!compact, |d| {
                                    d.child(
                                        div().text_size(px(11.)).text_color(content(0.7)).child(
                                            format!(
                                                "{}–{} of {count}",
                                                self.page * PAGE_SIZE + 1,
                                                ((self.page + 1) * PAGE_SIZE).min(count)
                                            ),
                                        ),
                                    )
                                })
                                .child(
                                    icon_button_enabled(
                                        "activity-next",
                                        "Next active sessions",
                                        "chevron-right",
                                        self.page + 1 < pages,
                                    )
                                    .debug_selector(|| "activity-next".into())
                                    .focusable()
                                    .tab_stop(true)
                                    .when(
                                        self.page + 1 < pages,
                                        |d| {
                                            d.on_click(cx.listener(|this, _, _, cx| {
                                                this.page += 1;
                                                this.scroll.set_offset(Point::default());
                                                this.sync_timer(cx);
                                                cx.notify();
                                            }))
                                        },
                                    ),
                                )
                                .when(compact, |d| d.flex_col()),
                        )
                    }),
            )
    }
}

impl Shell {
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
                let metadata = self.session_metadata(id, cx);
                if metadata.summary.archived || self.deleting.contains(&id) {
                    return None;
                }
                let status = if metadata.needs_approval() {
                    Status::Approval
                } else if metadata.responding()
                    || self.remote_marks.get(&id).is_some_and(|state| state.0)
                {
                    Status::Working
                } else if !metadata.busy && self.unseen_finished.contains(&id) {
                    Status::Done
                } else {
                    return None;
                };
                Some(Agent {
                    id,
                    title: metadata.display_title(),
                    project: self.project_label(&metadata.summary.cwd),
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
