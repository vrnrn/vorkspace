use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};

fn fixture(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64, i64) {
    vorkspace_ui::clock::use_test_clock(cx);
    super::super::tests::window(cx)
}
fn busy(window: WindowHandle<Shell>, id: i64, running: bool, cx: &mut TestAppContext) {
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&id].update(cx, |view, cx| {
                view.busy = running;
                cx.emit(SessionChanged);
                cx.notify();
            });
        })
        .unwrap();
    cx.run_until_parked();
}
fn snapshot(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> Vec<(i64, Status)> {
    window
        .update(cx, |shell, _, cx| {
            shell
                .live_agents
                .read(cx)
                .agents
                .iter()
                .map(|agent| (agent.id, agent.status))
                .collect()
        })
        .unwrap()
}
fn bounds(visual: &mut VisualTestContext, selector: String) -> Option<Bounds<Pixels>> {
    // The native test selector API requires a static name. This fixture
    // creates only a small, bounded set of row selectors.
    visual.debug_bounds(Box::leak(selector.into_boxed_str()))
}

#[gpui::test]
fn sessions_hang_under_their_projects_without_hydrating_history(cx: &mut TestAppContext) {
    let (window, a, b) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1300.), px(900.)));
    visual.run_until_parked();
    assert!(bounds(visual, format!("project-session-{a}")).is_none());
    assert!(visual.debug_bounds("sidebar-activity-card").is_none());
    busy(window, a, true, visual);
    busy(window, b, true, visual);
    let (a_project, b_project) = window
        .update(visual, |shell, _, cx| {
            let ix = |id: i64| {
                let cwd = shell.session_metadata(id, cx).summary.cwd.clone();
                shell
                    .workspace
                    .projects
                    .iter()
                    .position(|p| *p == cwd)
                    .unwrap()
            };
            (ix(a), ix(b))
        })
        .unwrap();
    assert_ne!(a_project, b_project);
    for (id, project) in [(a, a_project), (b, b_project)] {
        let row = bounds(visual, format!("project-session-{id}")).unwrap();
        let mark = bounds(visual, format!("classic-project-{project}")).unwrap();
        assert!(
            row.top() >= mark.bottom(),
            "a session hangs below its project"
        );
        assert!(bounds(visual, format!("classic-project-working-{project}")).is_some());
    }
    // The Working card is only for the icon rail.
    assert!(visual.debug_bounds("sidebar-activity-card").is_none());
    // Listing another project's session never loads its saved transcript.
    window
        .update(visual, |shell, _, cx| {
            assert!(!shell.sessions[&a].read(cx).history_loaded())
        })
        .unwrap();
    // Opening it selects exactly that session.
    let row = bounds(visual, format!("project-session-{a}")).unwrap();
    visual.simulate_click(row.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(shell.workspace.active, Some(a));
            assert_eq!(
                shell.workspace.project,
                shell.session_metadata(a, cx).summary.cwd.clone()
            );
        })
        .unwrap();
}

#[gpui::test]
fn the_activity_preferences_hide_project_trees(cx: &mut TestAppContext) {
    let (window, _, b) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    busy(window, b, true, visual);
    assert!(bounds(visual, format!("project-session-{b}")).is_some());
    window
        .update(visual, |shell, _, cx| {
            shell.change_sidebar_option("sidebar-activity", cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(bounds(visual, format!("project-session-{b}")).is_none());
    window
        .update(visual, |shell, _, cx| {
            shell.change_sidebar_option("sidebar-activity", cx);
            shell.workspace.preferences.working_agents = false;
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(bounds(visual, format!("project-session-{b}")).is_none());
}

#[gpui::test]
fn compact_activity_is_one_animated_control_for_working_sessions_and_expands(
    cx: &mut TestAppContext,
) {
    let (window, a, b) = fixture(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.workspace.project_rail_open = false;
            shell.workspace.preferences.collapsed_project_rail = "compact".into();
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1300.), px(900.)));
    visual.run_until_parked();
    assert!(visual.debug_bounds("compact-working").is_none());
    busy(window, a, true, visual);
    busy(window, b, true, visual);
    assert!(visual.debug_bounds("compact-working").is_some());
    assert!(bounds(visual, format!("live-agent-{a}")).is_none());
    assert!(bounds(visual, format!("live-agent-{b}")).is_none());
    let activity = window
        .update(visual, |shell, _, _| shell.live_agents.clone())
        .unwrap();
    assert_eq!(
        activity.read_with(visual, |card, _| card.working_count()),
        2
    );
    let frames = activity.read_with(visual, |card, _| card.frames);
    visual.executor().advance_clock(FRAME_TICK * 3);
    visual.run_until_parked();
    assert!(activity.read_with(visual, |card, _| card.frames) > frames);
    busy(window, a, false, visual);
    assert_eq!(
        activity.read_with(visual, |card, _| card.working_count()),
        1
    );
    let control = visual.debug_bounds("compact-working").unwrap();
    visual.simulate_click(control.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(shell.workspace.project_rail_open)
        })
        .unwrap();
    // Expanded, the session hangs under its project instead of the card.
    assert!(bounds(visual, format!("project-session-{b}")).is_some());
    window
        .update(visual, |shell, _, cx| shell.toggle_project_rail(cx))
        .unwrap();
    visual.run_until_parked();
    busy(window, b, false, visual);
    assert!(visual.debug_bounds("compact-working").is_none());
    assert!(visual.debug_bounds("sidebar-activity-card").is_none());
    assert!(activity.read_with(visual, |card, _| card.animation.is_none()));
}

#[gpui::test]
fn fast_completion_archive_and_cancellation_follow_semantic_notifications(cx: &mut TestAppContext) {
    let (window, a, b) = fixture(cx);
    window
        .update(cx, |shell, _, cx| {
            // Publish both changes before painting another frame.
            shell.sessions[&a].update(cx, |view, cx| {
                view.busy = true;
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&a].update(cx, |view, cx| {
                view.busy = false;
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(snapshot(window, cx).contains(&(a, Status::Done)));
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&a].update(cx, |view, cx| {
                view.summary.archived = true;
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(snapshot(window, cx).is_empty());
    busy(window, b, true, cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&b].update(cx, |view, cx| {
                view.stop_if_busy(cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert!(snapshot(window, cx).is_empty());
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.unseen_finished.contains(&b))
        })
        .unwrap();
}

#[gpui::test]
fn metadata_only_remote_running_and_waiting_sessions_do_not_load_history(cx: &mut TestAppContext) {
    let (window, _, _) = fixture(cx);
    let remote = window
        .update(cx, |shell, _, cx| {
            let store = locked(&shell.store).unwrap();
            let project = store
                .remember_remote_project(
                    "fixture",
                    &serde_json::json!({"id":"project", "cwd":"/host/repo"}),
                )
                .unwrap();
            let mut summary = store.create("codex", &project.key).unwrap();
            drop(store);
            summary.has_messages = true;
            let id = summary.id;
            shell.order.push(id);
            shell.session_summaries.insert(id, summary);
            shell.remote_marks.insert(id, (true, false));
            shell.invalidate_chrome(cx);
            id
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(snapshot(window, cx), vec![(remote, Status::Working)]);
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.sessions.contains_key(&remote));
            shell.remote_marks.insert(remote, (true, true));
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(snapshot(window, cx), vec![(remote, Status::Approval)]);
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.sessions.contains_key(&remote));
            shell.remote_marks.insert(remote, (false, false));
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(snapshot(window, cx).is_empty());
}

#[test]
#[ignore = "Requires Metal and VOKKY_ACTIVITY_CAPTURE_DIR; separate offscreen fixture only"]
fn native_activity_capture() {
    use vorkspace_core::store::Store;
    let output = std::path::PathBuf::from(std::env::var_os("VOKKY_ACTIVITY_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("native.sqlite")).unwrap();
    let summaries: Vec<_> = (0..6)
        .map(|i| {
            let mut summary = store
                .create(
                    if i % 2 == 0 { "codex" } else { "claude" },
                    if i % 2 == 0 {
                        "/work/vorkspace"
                    } else {
                        "/work/another-project"
                    },
                )
                .unwrap();
            summary.title = [
                "Restore sidebar activity",
                "Review provider ownership",
                "Verify app CLI launches",
                "Update component tokens",
                "Fix delegated session state",
                "Inspect the native layout",
            ][i]
                .into();
            summary.has_messages = true;
            summary
        })
        .collect();
    let store = Arc::new(Mutex::new(store));
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    cx.update(|cx| vorkspace_ui::clock::use_test_clock(cx));
    for theme in ["dark", "light", "harbor"] {
        for (name, compact, expanded, height, single) in [
            ("expanded", false, false, 800., false),
            ("compact", true, false, 800., false),
            ("all", false, true, 800., false),
            ("bottom", false, true, 800., false),
            ("short", false, true, 520., false),
            ("compact-all", true, true, 800., false),
            ("compact-short", true, true, 520., false),
            ("single", false, false, 800., true),
            ("plain", false, false, 800., true),
            ("done", false, false, 800., true),
            ("motion", false, false, 800., true),
        ] {
            if theme != "dark" && !matches!(name, "expanded" | "compact" | "plain" | "done") {
                continue;
            }
            // TestWindow::resize only changes its backing image size. Opening
            // at the target viewport gives honest native short-window captures.
            let window = cx
                .open_window(gpui::size(px(1280.), px(height)), |_, cx| {
                    cx.new(|cx| {
                        Shell::new(
                            store.clone(),
                            summaries.clone(),
                            "/work/vorkspace".into(),
                            None,
                            cx,
                        )
                    })
                })
                .unwrap();
            cx.run_until_parked();
            window
                .update(&mut cx, |shell, _, cx| {
                    for (ix, id) in shell.order.clone().into_iter().enumerate() {
                        let view = shell.ensure_session(id, cx).unwrap();
                        view.update(cx, |view, cx| {
                            view.busy = !single || ix == 2;
                            view.activity = if matches!(name, "plain" | "done" | "motion") {
                                None
                            } else {
                                Some(
                                    if view.summary.provider == "codex" {
                                        "Run cargo test"
                                    } else {
                                        "Read src/main.rs"
                                    }
                                    .into(),
                                )
                            };
                            cx.emit(SessionChanged);
                        });
                    }
                    shell
                        .resources
                        .update(cx, |resources, cx| resources.saturate(cx));
                })
                .unwrap();
            cx.run_until_parked();
            window
                .update(&mut cx, |shell, _, cx| {
                    if !single {
                        let id = shell.order[0];
                        shell.sessions[&id]
                            .update(cx, |view, cx| view.sidebar_test_approval(true, cx));
                        let id = shell.order[1];
                        shell.sessions[&id].update(cx, |view, cx| {
                            view.busy = false;
                            cx.emit(SessionChanged);
                        });
                    }
                    if name == "done" {
                        let id = shell.order[2];
                        shell.sessions[&id].update(cx, |view, cx| {
                            view.busy = false;
                            cx.emit(SessionChanged);
                        });
                    }
                    shell.workspace.preferences.theme =
                        if theme == "light" { "light" } else { "dark" }.into();
                    shell.workspace.preferences.workspace_theme = if theme == "harbor" {
                        "harbor"
                    } else {
                        "default"
                    }
                    .into();
                    shell.workspace.project_rail_open = !compact;
                    shell.live_agents.update(cx, |activity, cx| {
                        if activity.expanded != expanded {
                            activity.toggle_expanded(cx);
                        }
                    });
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            if name == "bottom" {
                window
                    .update(&mut cx, |shell, _, cx| {
                        shell.live_agents.update(cx, |activity, cx| {
                            activity.scroll.scroll_to_end();
                            cx.notify();
                        })
                    })
                    .unwrap();
                cx.run_until_parked();
            }
            if name == "motion" {
                window
                    .update(&mut cx, |shell, _, cx| {
                        for mark in shell.session_marks.values().chain(shell.tab_marks.values()) {
                            mark.update(cx, |mark, cx| mark.set("codex", MarkState::Idle, cx));
                        }
                    })
                    .unwrap();
            }
            vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("working-{theme}-{name}.png")))
                .unwrap();
            if name == "motion" {
                let first = cx.capture_screenshot(window.into()).unwrap();
                cx.advance_clock(Duration::from_millis(165));
                cx.run_until_parked();
                let second = cx.capture_screenshot(window.into()).unwrap();
                second
                    .save(output.join("working-dark-motion-next.png"))
                    .unwrap();
                assert!(
                    first.as_raw() != second.as_raw(),
                    "working grid must change painted pixels"
                );
            }
            window.update(&mut cx, |_, w, _| w.remove_window()).unwrap();
        }
    }
}

#[test]
fn historical_elapsed_format_handles_rounding_and_whole_units() {
    for (ms, label) in [
        (0, "1s"),
        (499, "1s"),
        (1499, "1s"),
        (1500, "2s"),
        (60_000, "1m"),
        (61_000, "1m 1s"),
        (3_600_000, "1h"),
        (3_660_000, "1h 1m"),
    ] {
        assert_eq!(live_elapsed(Duration::from_millis(ms)), label);
    }
}

#[gpui::test]
fn tool_notifications_update_only_the_receipt_preserving_focus_draft_and_run_clock(
    cx: &mut TestAppContext,
) {
    use crate::conversation::receive_tests::sidebar_tool_update;
    let (window, a, b) = fixture(cx);
    let tx = window
        .update(cx, |shell, w, cx| {
            let input = shell.sessions[&b].read(cx).input.clone();
            input.update(cx, |input, cx| {
                input.set_text("Keep my unfinished thought".into(), cx)
            });
            w.focus(&input.focus_handle(cx), cx);
            shell.sessions[&a].update(cx, |view, cx| view.sidebar_test_receiver(cx))
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let focus = visual.update(|w, cx| w.focused(cx));
    let (snapshots, started) = window
        .update(visual, |shell, _, cx| {
            let activity = shell.live_agents.read(cx);
            (activity.snapshots, activity.runs[&a].started)
        })
        .unwrap();
    for label in [
        Some("Run cargo check"),
        Some("Read src/main.rs"),
        Some("Read src/main.rs"),
        None,
    ] {
        tx.try_send(sidebar_tool_update(label)).unwrap();
        visual.executor().advance_clock(Duration::from_millis(1));
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.sessions[&a].read(cx).activity.as_deref(), label);
                let activity = shell.live_agents.read(cx);
                assert_eq!(activity.activity_label(&activity.agents[0]), label);
                assert_eq!(
                    activity.snapshots, snapshots,
                    "tool and output receipts must not resnapshot the registry"
                );
                assert_eq!(activity.runs[&a].started, started);
                assert_eq!(shell.workspace.active, Some(b));
                assert_eq!(
                    shell.sessions[&b].read(cx).input.read(cx).content,
                    "Keep my unfinished thought"
                );
            })
            .unwrap();
        assert_eq!(visual.update(|w, cx| w.focused(cx)), focus);
    }
    // Replacing the run revokes the previous receiver's authority, even if it
    // publishes a plausible label after a new run has started.
    let successor = window
        .update(visual, |shell, _, cx| {
            shell.sessions[&a].update(cx, |view, cx| view.sidebar_test_receiver(cx))
        })
        .unwrap();
    tx.try_send(sidebar_tool_update(Some("Run stale command")))
        .unwrap();
    successor
        .try_send(sidebar_tool_update(Some("Run current command")))
        .unwrap();
    visual.executor().advance_clock(Duration::from_millis(1));
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            let activity = shell.live_agents.read(cx);
            assert_eq!(
                activity.activity_label(&activity.agents[0]),
                Some("Run current command")
            );
        })
        .unwrap();
}

fn sample_agent(id: i64, status: Status, activity: Option<&str>) -> Agent {
    Agent {
        id,
        title: format!("Session {id}"),
        project: "vorkspace".into(),
        cwd: "/work/vorkspace".into(),
        appearance: Default::default(),
        activity: activity.map(str::to_owned),
        provider: "codex".into(),
        status,
        selected: false,
        updated_at: id as u64,
    }
}

struct MotionHost {
    card: Entity<LiveAgents>,
    renders: usize,
}
impl Render for MotionHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div().size_full().flex().flex_col().child(self.card.clone())
    }
}
fn motion_fixture(cx: &mut TestAppContext, count: usize) -> WindowHandle<MotionHost> {
    vorkspace_ui::clock::use_test_clock(cx);
    cx.add_window(|_, cx| {
        let card = cx.new(|cx| {
            let mut card = LiveAgents::new(gpui::WeakEntity::new_invalid(), cx);
            card.publish(
                (0..count)
                    .map(|id| sample_agent(id as i64, Status::Working, None))
                    .collect(),
                cx,
            );
            card.present(false, cx);
            card
        });
        MotionHost { card, renders: 0 }
    })
}

#[gpui::test]
fn metadata_status_precedes_provider_without_placeholder_lines_and_keeps_accessible_state(
    cx: &mut TestAppContext,
) {
    let window = motion_fixture(cx, 1);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    for (status, activity, label, height) in [
        (Status::Working, None, None, 48.),
        (Status::Working, Some("Working"), None, 48.),
        (Status::Working, Some("Done"), None, 48.),
        (Status::Working, Some("  "), None, 48.),
        (
            Status::Working,
            Some("Run cargo test"),
            Some("Run cargo test"),
            66.,
        ),
        (
            Status::Approval,
            Some("Run cargo test"),
            Some("Need approval"),
            66.,
        ),
        (Status::Done, Some("Run cargo test"), None, 48.),
    ] {
        window
            .update(visual, |host, _, cx| {
                host.card.update(cx, |card, cx| {
                    card.publish(vec![sample_agent(0, status, activity)], cx)
                })
            })
            .unwrap();
        visual.run_until_parked();
        let row = visual.debug_bounds("live-agent-0").unwrap();
        let title = visual.debug_bounds("live-agent-0-title").unwrap();
        let provider = visual.debug_bounds("live-agent-0-provider").unwrap();
        let mark = bounds(visual, format!("live-agent-0-{status:?}").to_lowercase()).unwrap();
        assert_eq!(row.size.height, px(height));
        assert_eq!(mark.center().y, provider.center().y);
        assert_eq!(provider.left() - mark.right(), px(6.));
        assert!(title.bottom() < mark.top());
        assert_eq!(
            visual.debug_bounds("live-agent-0-activity").is_some(),
            label.is_some()
        );
        window
            .update(visual, |host, _, cx| {
                let card = host.card.read(cx);
                assert_eq!(card.activity_label(&card.agents[0]), label);
                // Exercise the actual row element's AccessKit properties. The
                // test platform has no active screen reader/adapter tree.
                use gpui::Element;
                let row = card.row(&card.agents[0]);
                let mut node = gpui::accesskit::Node::new(row.a11y_role().unwrap());
                row.write_a11y_info(&mut node);
                assert_eq!(node.role(), Role::Button);
                assert!(
                    node.label()
                        .unwrap()
                        .starts_with(&format!("Session 0, vorkspace, {}", status.label()))
                );
                if let Some(label) = label {
                    assert!(node.label().unwrap().contains(label));
                }
            })
            .unwrap();
    }
}

#[gpui::test]
fn shared_frames_are_bounded_and_visibility_completion_and_disposal_cancel_timers(
    cx: &mut TestAppContext,
) {
    for count in [1, 100] {
        let window = motion_fixture(cx, count);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let (card, renders) = window
            .update(visual, |host, _, _| (host.card.clone(), host.renders))
            .unwrap();
        let (frames, snapshots, started, offset) = card.read_with(visual, |card, _| {
            assert!(card.animation.is_some());
            (
                card.frames,
                card.snapshots,
                card.runs[&0].started,
                card.scroll.logical_scroll_top(),
            )
        });
        let mut phases = Vec::new();
        for _ in 0..30 {
            visual.executor().advance_clock(FRAME_TICK);
            visual.run_until_parked();
            phases.push(card.read_with(visual, |card, _| card.frame_ms() % 1200));
        }
        card.read_with(visual, |card, _| {
            assert_eq!(
                card.frames - frames,
                30,
                "one shared tick regardless of row count"
            );
            assert_eq!(
                card.snapshots, snapshots,
                "frames cannot rebuild session discovery"
            );
            assert_eq!(card.runs[&0].started, started);
            assert_eq!(card.scroll.logical_scroll_top().item_ix, offset.item_ix);
            assert_eq!(
                card.scroll.logical_scroll_top().offset_in_item,
                offset.offset_in_item
            );
        });
        assert!(phases.windows(2).all(|pair| pair[0] != pair[1]));
        let redraws = window
            .update(visual, |host, _, _| host.renders - renders)
            .unwrap();
        assert_eq!(
            redraws, 30,
            "GPUI redraws the ancestor once per shared frame"
        );
        eprintln!(
            "working motion: rows={count}, frames=30, ancestor redraws={redraws}, discovery publications=0"
        );
        for mode in ["hidden-window", "approval", "done", "hidden-card"] {
            visual.simulate_visibility_change(if mode == "hidden-window" {
                gpui::WindowVisibility::Hidden
            } else {
                gpui::WindowVisibility::Visible
            });
            window
                .update(visual, |host, _, cx| {
                    host.card.update(cx, |card, cx| {
                        card.publish(
                            vec![sample_agent(
                                0,
                                if mode == "approval" {
                                    Status::Approval
                                } else if mode == "done" {
                                    Status::Done
                                } else {
                                    Status::Working
                                },
                                None,
                            )],
                            cx,
                        );
                        card.present(mode == "compact", cx);
                        if mode == "hidden-card" {
                            card.hide();
                        }
                    })
                })
                .unwrap();
            visual.run_until_parked();
            let before = card.read_with(visual, |card, _| {
                assert!(card.animation.is_none(), "{mode}");
                card.frames
            });
            visual.executor().advance_clock(FRAME_TICK * 3);
            visual.run_until_parked();
            assert_eq!(
                card.read_with(visual, |card, _| card.frames),
                before,
                "{mode}"
            );
        }
        card.update(visual, |card, cx| {
            card.present(false, cx);
        });
        visual.run_until_parked();
        card.read_with(visual, |card, _| assert!(card.animation.is_some()));
        let weak = card.downgrade();
        drop(card);
        window.update(visual, |_, w, _| w.remove_window()).unwrap();
        visual.run_until_parked();
        assert!(
            weak.upgrade().is_none(),
            "a task must not retain its unmounted owner"
        );
    }
}

#[gpui::test]
fn scrolling_to_done_only_viewport_parks_shared_motion_and_resumes_on_return(
    cx: &mut TestAppContext,
) {
    let window = motion_fixture(cx, 100);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let card = window
        .update(visual, |host, _, cx| {
            host.card.update(cx, |card, cx| {
                card.publish(
                    (0..100)
                        .map(|id| {
                            sample_agent(
                                id,
                                if id < 10 {
                                    Status::Working
                                } else {
                                    Status::Done
                                },
                                None,
                            )
                        })
                        .collect(),
                    cx,
                );
                card.toggle_expanded(cx);
            });
            host.card.clone()
        })
        .unwrap();
    visual.run_until_parked();
    card.update(visual, |card, cx| {
        card.scroll.scroll_to_end();
        cx.notify();
    });
    visual.run_until_parked();
    card.read_with(visual, |card, _| {
        assert!(card.animation.is_none());
        assert!(card.tick.is_none());
    });
    card.update(visual, |card, cx| {
        card.scroll.scroll_to(ListOffset::default());
        cx.notify();
    });
    visual.run_until_parked();
    card.read_with(visual, |card, _| assert!(card.animation.is_some()));
}

#[gpui::test]
fn mixed_height_activity_updates_and_reordering_keep_the_semantic_scroll_anchor(
    cx: &mut TestAppContext,
) {
    let window = motion_fixture(cx, 100);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1300.), px(300.)));
    visual.run_until_parked();
    let card = window
        .update(visual, |host, _, cx| {
            host.card.update(cx, |card, cx| {
                card.scroll.scroll_to(ListOffset {
                    item_ix: 1,
                    offset_in_item: px(7.),
                });
                cx.notify();
            });
            host.card.clone()
        })
        .unwrap();
    visual.run_until_parked();
    card.update(visual, |card, cx| card.toggle_expanded(cx));
    visual.run_until_parked();
    card.read_with(visual, |card, _| {
        let top = card.scroll.logical_scroll_top();
        assert_eq!(
            card.agents[top.item_ix].id, 1,
            "expansion keeps the scrolled session"
        );
        assert_eq!(top.offset_in_item, px(7.));
    });
    card.update(visual, |card, cx| {
        card.scroll.scroll_to(ListOffset {
            item_ix: 20,
            offset_in_item: px(7.),
        });
        cx.notify();
    });
    visual.run_until_parked();
    for label in [Some("Run cargo test"), None, Some("Read src/main.rs")] {
        card.update(visual, |card, cx| {
            card.set_activity(20, label.map(str::to_owned), cx)
        });
        visual.run_until_parked();
        card.read_with(visual, |card, _| {
            let top = card.scroll.logical_scroll_top();
            assert_eq!(card.agents[top.item_ix].id, 20);
            assert_eq!(top.offset_in_item, px(7.));
        });
    }
    card.update(visual, |card, cx| {
        let mut agents = card.agents.clone();
        agents[0].status = Status::Done;
        card.publish(agents, cx);
    });
    visual.run_until_parked();
    card.read_with(visual, |card, _| {
        let top = card.scroll.logical_scroll_top();
        assert_eq!(card.agents[top.item_ix].id, 20);
        assert_eq!(top.offset_in_item, px(7.));
        assert!(card.expanded);
    });
}

#[gpui::test]
fn removing_the_scrolled_session_keeps_the_nearest_surviving_session(cx: &mut TestAppContext) {
    for (removed, expected) in [
        (vec![20], 21),
        (vec![20, 21], 19),
        (vec![0, 1, 2, 3, 4, 20], 21),
        (vec![18, 19, 20, 21], 22),
    ] {
        let window = motion_fixture(cx, 40);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let card = window
            .update(visual, |host, _, cx| {
                host.card.update(cx, |card, cx| {
                    // Done entries disappear when they are opened and marked
                    // seen; archiving any session uses the same publication.
                    card.publish(
                        (0..40)
                            .map(|id| sample_agent(id, Status::Done, None))
                            .collect(),
                        cx,
                    );
                    card.toggle_expanded(cx);
                    card.scroll.scroll_to(ListOffset {
                        item_ix: 20,
                        offset_in_item: px(7.),
                    });
                    cx.notify();
                });
                host.card.clone()
            })
            .unwrap();
        visual.run_until_parked();
        card.update(visual, |card, cx| {
            let agents = card
                .agents
                .iter()
                .filter(|agent| !removed.contains(&agent.id))
                .cloned()
                .collect();
            card.publish(agents, cx);
        });
        visual.run_until_parked();
        card.read_with(visual, |card, _| {
            let top = card.scroll.logical_scroll_top();
            assert_eq!(card.agents[top.item_ix].id, expected, "removed={removed:?}");
            assert_eq!(top.offset_in_item, px(7.));
            assert!(card.expanded);
        });
        // An empty list has no surviving anchor and must reset disclosure.
        card.update(visual, |card, cx| card.publish(Vec::new(), cx));
        visual.run_until_parked();
        card.read_with(visual, |card, _| {
            assert!(!card.expanded);
            let top = card.scroll.logical_scroll_top();
            assert_eq!(top.item_ix, 0);
            assert_eq!(top.offset_in_item, px(0.));
        });
    }
}
