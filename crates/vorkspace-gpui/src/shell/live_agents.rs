//! A bounded projection of Shell's semantic session registry. The card owns its
//! presentation clock: ticking never hydrates history or rebuilds the session snapshot.
use super::*;
use std::time::Duration;
use vorkspace_ui::clock::Instant;
use vorkspace_ui::radius;

const FRAME_TICK: Duration = Duration::from_millis(33);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Status {
    Approval,
    Working,
    Done,
}
#[derive(Clone, Debug, PartialEq)]
struct Agent {
    id: i64,
    status: Status,
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
    visible: bool,
    window_visible: bool,
    visibility_subscription: Option<Subscription>,
    animation: Option<gpui::Task<()>>,
    animation_started: Instant,
    #[cfg(test)]
    frames: usize,
    #[cfg(test)]
    snapshots: usize,
}

impl LiveAgents {
    pub(super) fn new(owner: gpui::WeakEntity<Shell>) -> Self {
        Self {
            owner,
            agents: Vec::new(),
            runs: HashMap::new(),
            visible: false,
            window_visible: false,
            visibility_subscription: None,
            animation: None,
            animation_started: Instant::now(),
            #[cfg(test)]
            frames: 0,
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
        self.animation = None;
    }

    fn working_count(&self) -> usize {
        self.agents
            .iter()
            .filter(|agent| agent.status == Status::Working)
            .count()
    }

    fn animation_visible(&self) -> bool {
        self.visible && self.window_visible && self.working_count() > 0
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
            self.agents = agents;
            cx.notify();
        }
        self.sync_animation(cx);
    }

    fn present(&mut self, cx: &mut Context<Self>) -> bool {
        if self.agents.is_empty() || self.working_count() == 0 {
            self.hide();
            return false;
        }
        if !self.visible {
            self.visible = true;
            cx.notify();
        }
        self.sync_animation(cx);
        true
    }
}

impl Render for LiveAgents {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.visibility_subscription.is_none() {
            self.window_visible = window.is_visible();
            self.visibility_subscription = Some(cx.observe_window_visibility(
                window,
                |this, visibility, _, cx| {
                    this.window_visible = visibility.is_visible();
                    this.sync_animation(cx);
                },
            ));
        }
        // The compact surface has no list prepaint callback to admit motion.
        self.sync_animation(cx);
        let count = self.working_count();
        let owner = self.owner.clone();
        div()
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
                .rounded(px(radius::CONTROL))
                .flex()
                .items_center()
                .justify_center()
                .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
                .child(vorkspace_ui::session_mark::spinner_dots(self.frame_ms()))
                .on_click(move |_, _, cx| {
                    let _ = owner.update(cx, |shell, cx| {
                        if !shell.workspace.project_rail_open {
                            shell.toggle_project_rail(cx);
                        }
                    });
                }),
            )
    }
}

impl Shell {
    /// Why a session belongs in the sidebar's activity: running, waiting on an
    /// approval, or finished unseen. Archived and deleting sessions never do.
    pub(super) fn sidebar_status(&self, id: i64, cx: &App) -> Option<Status> {
        if self.team_histories.borrow().contains_key(&id) {
            return None;
        }
        let metadata = self.session_metadata(id, cx);
        if metadata.summary.archived || self.deleting.contains(&id) {
            return None;
        }
        if metadata.needs_approval() {
            Some(Status::Approval)
        } else if metadata.responding() {
            Some(Status::Working)
        } else if !metadata.busy && self.unseen_finished.contains(&id) {
            Some(Status::Done)
        } else {
            None
        }
    }

    pub(super) fn sync_live_agents(&self, cx: &mut Context<Self>) {
        // App CLI and Bot launchers both install into this registry, independently
        // of tab/project selection and project folder collapse state.
        // Rendering this section never calls ensure_session.
        let ids: HashSet<_> = self
            .sessions
            .keys()
            .chain(self.unseen_finished.iter())
            .copied()
            .collect();
        let agents: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                if !self.has_session(id, cx) {
                    return None;
                }
                let status = self.sidebar_status(id, cx)?;
                let metadata = self.session_metadata(id, cx);
                Some(Agent {
                    id,
                    status,
                    updated_at: metadata.summary.updated_at,
                })
            })
            .collect();
        let hidden = self.classic_rail_width() <= 0.
            || self.pages.settings.is_some()
            || !self.workspace.sidebar_activity_visible
            || !self.workspace.preferences.working_agents;
        self.sidebar.live_agents.update(cx, |activity, cx| {
            activity.publish(agents, cx);
            // A fully hidden rail is unmounted, so its render callback cannot
            // be responsible for releasing the presentation timer.
            if hidden {
                activity.hide();
            }
        });
    }

    pub(super) fn live_agents_preview(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.workspace.sidebar_activity_visible || !self.workspace.preferences.working_agents {
            self.sidebar
                .live_agents
                .update(cx, |activity, _| activity.hide());
            return None;
        }
        let visible = self
            .sidebar
            .live_agents
            .update(cx, |activity, cx| activity.present(cx));
        visible.then(|| self.sidebar.live_agents.clone().into_any_element())
    }
}

#[cfg(test)]
#[path = "live_agents_tests.rs"]
mod tests;

#[cfg(test)]
mod minion_history_tests {
    use super::*;
    use gpui::TestAppContext;
    use std::sync::{Arc, Mutex};
    use vorkspace_core::{
        bot::{Bot, MinionMetadata},
        store::Store,
    };

    #[gpui::test]
    fn restored_team_views_never_join_project_activity_trees(cx: &mut TestAppContext) {
        vorkspace_ui::clock::use_test_clock(cx);
        let store = Store::test(std::path::Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/missing/team-project").unwrap();
        let id = session.id;
        let window = cx
            .update(|cx| {
                cx.open_window(Default::default(), |_, cx| {
                    cx.new(|cx| {
                        Shell::new(
                            Arc::new(Mutex::new(store)),
                            vec![session],
                            "/missing/team-project".into(),
                            None,
                            cx,
                        )
                    })
                })
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let view = shell.bot_ensure_session(id, cx).unwrap();
                view.update(cx, |view, _| view.set_running_for_test(true));
                shell.team_histories.borrow_mut().insert(
                    id,
                    crate::bots::TeamHistory {
                        actor: Bot {
                            id: "retired-member".into(),
                            session_id: Some(id),
                            minion: Some(MinionMetadata {
                                parent_id: "parent".into(),
                                role: "Reviewer".into(),
                                retired: true,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        parent: None,
                    },
                );
                shell.workspace.preferences.minions = false;
                assert!(view.read(cx).user_read_only());
                assert!(shell.sidebar_status(id, cx).is_none());
                shell.sync_live_agents(cx);
                assert!(
                    !shell
                        .sidebar
                        .live_agents
                        .read(cx)
                        .agents
                        .iter()
                        .any(|agent| agent.id == id)
                );
                view.update(cx, |view, _| view.set_running_for_test(false));
            })
            .unwrap();
    }
}
