//! Sidebar.tsx's rails: an orchestrator's agents (OrchestrationSidebarAgents,
//! `tree`) and the sessions an /operator thread started hang off a line under
//! its row. Each rail opens while its thread, or one of the thread's
//! sessions, is in view, chosen or working, and RevealHeight grows it in.
use super::*;
use crate::reveal::{Disclosure, bezier};
use std::time::{Duration, Instant};

/// RevealHeight's `reveal-height-in` and `reveal-height-out`.
const OPEN: Duration = Duration::from_millis(240);
const CLOSE: Duration = Duration::from_millis(180);

#[derive(Default)]
pub(crate) struct Rails {
    motions: HashMap<i64, Motion>,
    /// Agent rows opened to their details, by lead and task.
    open_tasks: HashSet<(i64, usize)>,
    /// This frame's /operator groups, and the sessions they took from the list.
    pub(super) started: HashMap<i64, Vec<i64>>,
    pub(super) children: HashSet<i64>,
    frames: Option<gpui::Task<()>>,
}
#[derive(Clone, Copy)]
struct Motion {
    open: bool,
    from: f32,
    start: Instant,
}
impl Motion {
    fn value(&self, now: Instant) -> f32 {
        let (duration, curve) = if self.open {
            (OPEN, (0.2, 0., 0., 1.))
        } else {
            (CLOSE, (0.4, 0., 1., 1.))
        };
        let t = (now.saturating_duration_since(self.start).as_secs_f32() / duration.as_secs_f32())
            .clamp(0., 1.);
        let eased = bezier(t, curve.0, curve.1, curve.2, curve.3);
        let target = if self.open { 1. } else { 0. };
        self.from + (target - self.from) * eased
    }
    fn settled(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) >= if self.open { OPEN } else { CLOSE }
    }
}

/// What hangs under a sidebar row.
pub(crate) enum Rail {
    /// Sessions this /operator thread started, newest first.
    Started(Vec<i64>),
    /// This orchestrator's agents.
    Agents(vorkspace_core::orchestration::Summary),
}

/// `.zen-phase-step.orchestration-tree-step`: a step branching off the
/// transcript's tool rail, its row 2px lower inside the step's padding.
pub(super) fn step(last: bool, child: impl IntoElement) -> gpui::Div {
    div()
        .relative()
        .pl(px(20.))
        .py(px(2.))
        .child(crate::activity::rail_branch_at(last, true, 2.))
        .child(child)
}

impl Shell {
    fn in_play(&self, id: i64, cx: &App) -> bool {
        (self.workspace.active == Some(id) && !self.bot_active(cx))
            || self.selected_sessions.contains(&id)
            || self
                .sessions
                .get(&id)
                .is_some_and(|s| s.read(cx).responding())
    }
    /// groupStartedSessions: each listed session another started joins the
    /// listed session at the top of its chain; one whose caller is not listed
    /// stays where it is.
    pub(super) fn started_groups(&self, ids: &[i64]) -> (HashMap<i64, Vec<i64>>, HashSet<i64>) {
        let listed: HashSet<i64> = ids.iter().copied().collect();
        let caller = |id: i64| {
            self.started_by
                .get(&id)
                .copied()
                .filter(|p| listed.contains(p))
        };
        let mut groups: HashMap<i64, Vec<i64>> = HashMap::new();
        let mut children = HashSet::new();
        for &id in ids {
            let Some(mut root) = caller(id) else {
                continue;
            };
            let mut seen = HashSet::from([id]);
            while let Some(up) = caller(root) {
                if !seen.insert(root) {
                    break;
                }
                root = up;
            }
            children.insert(id);
            groups.entry(root).or_default().push(id);
        }
        (groups, children)
    }
    /// Orchestrated workers live on their lead's rail, never in the list.
    pub(super) fn orchestration_worker(&self, id: i64) -> bool {
        self.orchestration_summaries
            .values()
            .any(|s| s.tasks.iter().any(|t| t.session == Some(id)))
    }
    fn rail(&self, id: i64, started: &HashMap<i64, Vec<i64>>) -> Option<Rail> {
        if let Some(summary) = self.orchestration_summaries.get(&id)
            && !summary.tasks.is_empty()
        {
            return Some(Rail::Agents(summary.clone()));
        }
        started
            .get(&id)
            .filter(|c| !c.is_empty())
            .map(|c| Rail::Started(c.clone()))
    }
    fn rail_open(&self, id: i64, rail: &Rail, cx: &App) -> bool {
        match rail {
            // orchestrationExpanded: the lead in view, chosen or working.
            Rail::Agents(_) => self.in_play(id, cx),
            Rail::Started(children) => {
                self.in_play(id, cx) || children.iter().any(|c| self.in_play(*c, cx))
            }
        }
    }
    /// Moves each rail toward its open state. A rail the sidebar first shows
    /// is simply there; later changes grow and fold.
    pub(super) fn sync_rails(&mut self, ids: &[i64], cx: &mut Context<Self>) {
        let (started, children) = self.started_groups(ids);
        let now = Instant::now();
        let mut animating = false;
        let mut seen = HashSet::new();
        for &id in ids {
            let Some(rail) = self.rail(id, &started) else {
                continue;
            };
            seen.insert(id);
            let open = self.rail_open(id, &rail, cx);
            let reduced = cx.reduce_motion();
            match self.rails.motions.get(&id).copied() {
                None => {
                    self.rails.motions.insert(
                        id,
                        Motion {
                            open,
                            from: if open { 1. } else { 0. },
                            start: now - OPEN,
                        },
                    );
                }
                Some(motion) if motion.open != open => {
                    let from = if reduced {
                        if open { 1. } else { 0. }
                    } else {
                        motion.value(now)
                    };
                    self.rails.motions.insert(
                        id,
                        Motion {
                            open,
                            from,
                            start: if reduced { now - OPEN } else { now },
                        },
                    );
                }
                _ => {}
            }
            animating |= !self.rails.motions[&id].settled(now);
            // A working agent's spinner turns on an open rail.
            if open
                && let Rail::Agents(summary) = &rail
                && summary.live
                && summary.tasks.iter().any(|t| t.status == "running")
            {
                animating = true;
            }
        }
        self.rails.motions.retain(|id, _| seen.contains(id));
        self.rails.started = started;
        self.rails.children = children;
        if animating && self.rails.frames.is_none() {
            self.rails.frames = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.rails.frames = None;
                    this.notify_sidebar(cx);
                });
            }));
        }
    }
    /// A session row and, under it, whatever rail it has.
    pub(super) fn session_card(&self, id: i64, cx: &mut Context<Self>) -> AnyElement {
        self.session_card_at_depth(id, false, cx)
    }

    /// Folder members use the same compact row as an Operator's children,
    /// while keeping any Operator or Orchestrator rail of their own.
    pub(super) fn nested_session_card(&self, id: i64, cx: &mut Context<Self>) -> AnyElement {
        self.session_card_at_depth(id, true, cx)
    }

    fn session_card_at_depth(&self, id: i64, nested: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(rail) = self.rail(id, &self.rails.started) else {
            return if nested {
                self.session_row_with(id, true, None, cx)
            } else {
                self.session_row(id, cx)
            };
        };
        let now = Instant::now();
        let progress = self.rails.motions.get(&id).map_or(0., |m| m.value(now));
        let open = self.rails.motions.get(&id).is_some_and(|m| m.open);
        let row = self.session_row_with(id, nested, Some((&rail, open)), cx);
        if progress <= 0. {
            return row;
        }
        let body = match &rail {
            Rail::Started(children) => div()
                .pt_1()
                .flex()
                .flex_col()
                .children(children.iter().enumerate().map(|(ix, child)| {
                    step(
                        ix + 1 == children.len(),
                        self.session_row_with(*child, true, None, cx),
                    )
                }))
                .into_any_element(),
            Rail::Agents(summary) => self.agents(id, summary, cx),
        };
        // The rail drops from under the row's mark.
        let width = px((self.workspace.right_panel_width
            - 16.
            - 10.
            - if nested { 30. } else { 0. })
        .max(0.));
        let body = div().ml(px(10.)).child(body);
        div()
            .flex()
            .flex_col()
            .child(row)
            .child(if progress >= 1. {
                body.into_any_element()
            } else {
                div()
                    .ml(px(10.))
                    .opacity(progress)
                    .overflow_hidden()
                    .child(Disclosure::new(body.ml(px(0.)), width, progress))
                    .into_any_element()
            })
            .into_any_element()
    }
    /// OrchestrationSidebarAgents, `tree`.
    fn agents(
        &self,
        lead: i64,
        summary: &vorkspace_core::orchestration::Summary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spin = vorkspace_core::time::now_ms();
        div()
            .pt_1()
            .pb_1()
            .flex()
            .flex_col()
            .children(summary.tasks.iter().enumerate().map(|(ix, task)| {
                let needs_input = task.session.is_some_and(|s| {
                    self.sessions
                        .get(&s)
                        .is_some_and(|v| v.read(cx).needs_approval())
                });
                let open = self.rails.open_tasks.contains(&(lead, ix)) || needs_input;
                let label = task.label(summary, needs_input);
                let working = summary.live && task.status == "running" && !needs_input;
                let alert = needs_input || matches!(task.status.as_str(), "failed" | "blocked");
                let done = task.status == "completed";
                let tone: gpui::Hsla = if alert {
                    rgb(0xfbbf24).into()
                } else if working {
                    accent_color()
                } else if done {
                    rgb(0x34d399).into()
                } else {
                    content(0.45)
                };
                let model = crate::pickers::model_name(
                    Kind::from_id(&task.provider),
                    task.model.as_deref(),
                )
                .to_string();
                let group: SharedString = format!("rail-agent-{lead}-{ix}").into();
                let header = div()
                    .id(("rail-agent", (lead as usize) << 8 | ix))
                    .debug_selector(move || format!("rail-agent-{lead}-{ix}"))
                    .role(Role::Button)
                    .aria_label(format!("Agent details: {}", task.title))
                    .aria_expanded(open)
                    .group(group.clone())
                    .h(px(28.))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(6.))
                    .rounded_md()
                    .cursor(CursorStyle::Arrow)
                    .when(open, |d| d.bg(content(0.1)))
                    .when(!open, |d| d.hover(|s| s.bg(content(0.1))))
                    .child(
                        div()
                            .size(px(14.))
                            .flex_shrink_0()
                            .opacity(0.75)
                            .child(provider_icon(&task.provider, 14.)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(content(if open { 1. } else { 0.55 }))
                            .when(!open, |d| {
                                d.group_hover(group.clone(), |s| s.text_color(content(0.8)))
                            })
                            .child(task.title.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(11.))
                            .text_color(tone)
                            .when(alert, |d| {
                                d.child(icon("circle-alert", 12.).text_color(tone))
                            })
                            .when(working, |d| {
                                d.child(crate::session_mark::spinner_dots(spin))
                            })
                            .when(done && !alert, |d| {
                                d.child(icon("check", 12.).text_color(tone))
                            })
                            // On the rail the spinner says it on its own.
                            .when(!working, |d| d.child(label)),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.rails.open_tasks.remove(&(lead, ix)) {
                            this.rails.open_tasks.insert((lead, ix));
                        }
                        this.notify_sidebar(cx);
                    }));
                let worker = task.session;
                let running = worker.is_some_and(|s| {
                    summary.live
                        && self
                            .sessions
                            .get(&s)
                            .is_some_and(|v| v.read(cx).responding())
                });
                let solid = |id: &'static str, label: &'static str| {
                    div()
                        .id((id, (lead as usize) << 8 | ix))
                        .role(Role::Button)
                        .aria_label(label)
                        .rounded(px(4.))
                        .bg(content(0.15))
                        .px(px(6.))
                        .py(px(2.))
                        .text_size(px(11.))
                        .text_color(content(0.75))
                        .cursor(CursorStyle::Arrow)
                        .hover(|s| s.bg(content(0.25)).text_color(content(1.)))
                        .child(label)
                };
                let details = div()
                    .pl(px(6.))
                    .pt(px(6.))
                    .pb_3()
                    .pr_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(provider_icon(&task.provider, 14.))
                            .child(div().min_w_0().truncate().child(model)),
                    )
                    .when(worker.is_some() || running, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_1()
                                .when_some(worker, |d, worker| {
                                    d.child(solid("rail-agent-open", "See details").on_click(
                                        cx.listener(move |this, _, window, cx| {
                                            this.open(worker, window, cx)
                                        }),
                                    ))
                                })
                                .when_some(worker.filter(|_| running), |d, worker| {
                                    d.child(solid("rail-agent-cancel", "Cancel task").on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            if let Some(view) = this.sessions.get(&worker) {
                                                view.update(cx, |v, cx| v.stop_if_busy(cx));
                                            }
                                        }),
                                    ))
                                }),
                        )
                    });
                step(
                    ix + 1 == summary.tasks.len(),
                    div()
                        .flex()
                        .flex_col()
                        .child(header)
                        .when(open, |d| d.child(details)),
                )
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn bounds(cx: &mut VisualTestContext, selector: String) -> Option<gpui::Bounds<Pixels>> {
        cx.debug_bounds(Box::leak(selector.into_boxed_str()))
    }
    /// Rail motion runs on the wall clock, as the window's frames do.
    fn settle(window: gpui::WindowHandle<Shell>, cx: &mut VisualTestContext) {
        for _ in 0..2 {
            std::thread::sleep(OPEN + Duration::from_millis(20));
            window
                .update(cx, |shell, _, cx| {
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
        }
    }

    #[gpui::test]
    fn operator_and_orchestrator_threads_hang_their_work_on_a_rail_that_follows_selection(
        cx: &mut TestAppContext,
    ) {
        let (window, _, lead) = crate::shell::tests::window(cx);
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        let (child, other, worker) = window
            .update(cx, |shell, _, cx| {
                let mut ids = Vec::new();
                for _ in 0..3 {
                    let summary = locked(&shell.store)
                        .unwrap()
                        .create("codex", &shell.workspace.project)
                        .unwrap();
                    ids.push(summary.id);
                    shell.add(summary, cx);
                }
                for id in ids.iter().copied().chain([lead]) {
                    shell.sessions[&id].update(cx, |s, _| s.summary.has_messages = true);
                }
                let (child, other, worker) = (ids[0], ids[1], ids[2]);
                shell.started_by.insert(child, lead);
                shell.orchestration_summaries.insert(
                    other,
                    vorkspace_core::orchestration::Summary {
                        live: false,
                        tasks: vec![vorkspace_core::orchestration::SummaryTask {
                            session: Some(worker),
                            title: "Write the tests".into(),
                            provider: "codex".into(),
                            model: None,
                            status: "completed".into(),
                        }],
                    },
                );
                shell.selected_sessions.clear();
                shell.workspace.active = Some(lead);
                shell.invalidate_chrome(cx);
                (child, other, worker)
            })
            .unwrap();
        settle(window, cx);
        let head = bounds(cx, format!("sidebar-session-{lead}")).expect("the thread is listed");
        let nested =
            bounds(cx, format!("sidebar-session-{child}")).expect("its session hangs below");
        assert!(nested.top() >= head.bottom(), "under its caller");
        assert!(nested.left() > head.left(), "on the rail, inset");
        assert_eq!(nested.size.height, px(28.));
        assert!(
            bounds(cx, format!("sidebar-session-{worker}")).is_none(),
            "a worker is never listed on its own"
        );
        assert!(
            bounds(cx, format!("rail-agent-{other}-0")).is_none(),
            "an orchestrator's rail is closed while it is out of play"
        );
        // Clicking the orchestrator opens its agents and folds the other rail;
        // the click does not also land on an agent that grows in under it.
        let row = bounds(cx, format!("sidebar-session-{other}")).unwrap();
        let clicked = Instant::now();
        cx.simulate_click(row.center(), Default::default());
        cx.run_until_parked();
        window
            .read_with(cx, |shell, _| {
                assert_eq!(shell.workspace.active, Some(other));
                assert!(shell.rails.open_tasks.is_empty());
            })
            .unwrap();
        let opening = window
            .read_with(cx, |shell, _| shell.rails.motions[&other])
            .unwrap();
        // A motion that starts at the click, rather than one already settled.
        assert!(
            opening.open && opening.start >= clicked,
            "the rail grows in"
        );
        assert!(opening.value(opening.start) < 0.01);
        settle(window, cx);
        let agent = bounds(cx, format!("rail-agent-{other}-0")).expect("its agents show");
        window
            .read_with(cx, |shell, _| assert!(shell.rails.open_tasks.is_empty()))
            .unwrap();
        cx.simulate_click(agent.center(), Default::default());
        cx.run_until_parked();
        window
            .read_with(cx, |shell, _| {
                assert!(
                    shell.rails.open_tasks.contains(&(other, 0)),
                    "a click opens its details"
                )
            })
            .unwrap();
        settle(window, cx);
        let agent = bounds(cx, format!("rail-agent-{other}-0")).unwrap();
        cx.simulate_click(agent.center(), Default::default());
        cx.run_until_parked();
        window
            .read_with(cx, |shell, _| {
                assert!(shell.rails.open_tasks.is_empty(), "and closes them")
            })
            .unwrap();
        assert!(
            bounds(cx, format!("sidebar-session-{child}")).is_none(),
            "the operator rail folds away once its thread leaves play"
        );
    }
}
