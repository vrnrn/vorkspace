use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use vorkspace_core::{agent::Settings, session_groups::Groups};

fn fixture(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64, i64) {
    crate::clock::use_test_clock(cx);
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
fn zero_one_and_multiple_sessions_are_discoverable_without_hydrating_history(
    cx: &mut TestAppContext,
) {
    let (window, a, b) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1300.), px(900.)));
    visual.run_until_parked();
    assert!(visual.debug_bounds("sidebar-activity-card").is_none());
    busy(window, a, true, visual);
    assert!(visual.debug_bounds("sidebar-activity-card").is_some());
    assert!(bounds(visual, format!("live-agent-{a}")).is_some());
    assert_eq!(snapshot(window, visual), vec![(a, Status::Working)]);
    busy(window, b, true, visual);
    assert_eq!(snapshot(window, visual).len(), 2);
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(shell.workspace.active, Some(b));
            assert_eq!(shell.workspace.project, "/work/vorkspace");
            // a belongs to another project, but neither enumeration nor ticking
            // loads its saved transcript.
            assert!(!shell.sessions[&a].read(cx).history_loaded());
        })
        .unwrap();
}

#[gpui::test]
fn approvals_sort_first_and_unseen_completions_open_the_exact_cross_project_session(
    cx: &mut TestAppContext,
) {
    let (window, a, b) = fixture(cx);
    busy(window, a, true, cx);
    busy(window, b, true, cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&b].update(cx, |view, cx| view.sidebar_test_approval(true, cx));
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert_eq!(
        snapshot(window, visual),
        vec![(b, Status::Approval), (a, Status::Working)]
    );
    assert!(bounds(visual, format!("live-agent-{b}-approval")).is_some());
    visual.executor().advance_clock(Duration::from_secs(3));
    busy(window, a, false, visual);
    assert_eq!(
        snapshot(window, visual),
        vec![(b, Status::Approval), (a, Status::Done)]
    );
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(
                shell
                    .live_agents
                    .read(cx)
                    .elapsed(&shell.live_agents.read(cx).agents[1]),
                Some("3s".into())
            );
        })
        .unwrap();
    let row = bounds(visual, format!("live-agent-{a}")).unwrap();
    visual.simulate_click(row.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(shell.workspace.active, Some(a));
            assert_eq!(shell.workspace.project, "/work/another-project");
            assert!(!shell.unseen_finished.contains(&a));
            assert!(shell.sessions[&b].read(cx).needs_approval());
            assert!(shell.sessions[&b].read(cx).responding());
        })
        .unwrap();
    assert!(!snapshot(window, visual).iter().any(|(id, _)| *id == a));
}

#[gpui::test]
fn visibility_menu_persists_and_respects_legacy_opt_out_without_changing_other_settings(
    cx: &mut TestAppContext,
) {
    let (window, a, _) = fixture(cx);
    busy(window, a, true, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let options = visual.debug_bounds("sidebar-options").unwrap();
    visual.simulate_click(options.center(), Default::default());
    visual.run_until_parked();
    crate::clock::settle_animations(window, visual);
    let row = visual.debug_bounds("menu-action-sidebar-activity").unwrap();
    visual.simulate_click(row.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("sidebar-activity-card").is_none());
    assert!(
        visual
            .debug_bounds("menu-action-sidebar-activity")
            .is_some()
    );
    window
        .update(visual, |shell, _, cx| {
            assert!(!shell.workspace.sidebar_activity_visible);
            let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
            let restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
            assert!(!restored.sidebar_activity_visible);
            assert_eq!(restored.preferences, shell.workspace.preferences);
            shell.workspace.sidebar_activity_visible = true;
            shell.workspace.preferences.working_agents = false;
            shell.close_menu(cx);
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("sidebar-activity-card").is_none());
    window
        .update(visual, |shell, _, cx| {
            let active = shell.workspace.active;
            let selected = shell.selected_sessions.clone();
            shell.change_sidebar_option("sidebar-activity", cx);
            assert!(shell.workspace.preferences.working_agents);
            assert!(shell.workspace.sidebar_activity_visible);
            assert_eq!(shell.workspace.active, active);
            assert_eq!(shell.selected_sessions, selected);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("sidebar-activity-card").is_some());
    visual.simulate_resize(gpui::size(px(1300.), px(1100.)));
    window
        .update(visual, |shell, w, cx| {
            shell.open_settings(crate::settings::Section::General, w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    crate::clock::settle_animations(window, visual);
    for enabled in [false, true] {
        let toggle = visual
            .debug_bounds("settings-toggle-working-agents")
            .unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.preferences.working_agents, enabled);
                let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
                let restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
                assert_eq!(restored.preferences.working_agents, enabled);
            })
            .unwrap();
    }
    window
        .update(visual, |shell, w, cx| shell.close_settings(w, cx))
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("sidebar-activity-card").is_some());
}

#[gpui::test]
fn cli_and_bot_admissions_and_group_updates_remain_visible_while_a_bot_covers_the_workspace(
    cx: &mut TestAppContext,
) {
    let (window, caller, underlying) = fixture(cx);
    let (cli, bot) = window
        .update(cx, |shell, w, cx| {
            shell.sessions[&caller].update(cx, |view, cx| view.enable_mode("operator", cx));
            let summary = locked(&shell.store)
                .unwrap()
                .create("codex", "/work/another-project")
                .unwrap();
            let cli = summary.id;
            assert_eq!(
                shell
                    .app_install_start(
                        caller,
                        app_commands::AppStart {
                            summary,
                            settings: Settings::default(),
                            prompt: "Preserve CLI draft".into(),
                            draft: true,
                            reveal: false,
                            placement: None,
                            cleanup: None,
                        },
                        w,
                        cx
                    )
                    .unwrap(),
                cli
            );
            let summary = locked(&shell.store)
                .unwrap()
                .create("claude", "/work/bot-project")
                .unwrap();
            let bot = summary.id;
            shell.bot_install(
                summary,
                Settings::default(),
                "Preserve Bot draft".into(),
                true,
                cx,
            );
            let manager = shell.bot_manager();
            manager.update(cx, |manager, cx| {
                manager.create_named(Vec::new(), w, cx);
                manager.dismiss_settings(w, cx);
            });
            shell.selected_sessions.insert(caller);
            (cli, bot)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            shell.selected_sessions.insert(caller);
        })
        .unwrap();
    for id in [cli, bot, underlying] {
        busy(window, id, true, cx);
    }
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert_eq!(snapshot(window, visual).len(), 3);
    window
        .update(visual, |shell, _, cx| {
            assert!(shell.bot_active(cx));
            assert_eq!(shell.workspace.active, Some(underlying));
            assert!(
                shell
                    .live_agents
                    .read(cx)
                    .agents
                    .iter()
                    .all(|agent| !agent.selected)
            );
            let before = shell.live_agents.read(cx).runs[&cli].started;
            let mut groups = Groups::default();
            groups.create_folder(&[cli], "delegated".into());
            groups.folders[0].collapsed = true;
            shell.bot_apply_groups("/work/another-project".into(), groups, cx);
            assert_eq!(shell.live_agents.read(cx).runs[&cli].started, before);
        })
        .unwrap();
    visual.run_until_parked();
    busy(window, underlying, false, visual);
    assert!(snapshot(window, visual).contains(&(underlying, Status::Done)));
    window
        .update(visual, |shell, _, cx| {
            assert!(
                shell.unseen_finished.contains(&underlying),
                "a Bot covers the selected session"
            );
            assert!(shell.selected_sessions.contains(&caller));
            for (id, text) in [(cli, "Preserve CLI draft"), (bot, "Preserve Bot draft")] {
                assert_eq!(shell.sessions[&id].read(cx).input.read(cx).content, text);
            }
        })
        .unwrap();
    let row = bounds(visual, format!("live-agent-{cli}")).unwrap();
    visual.simulate_click(row.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert!(!shell.bot_active(cx));
            assert_eq!(shell.workspace.active, Some(cli));
            assert_eq!(shell.workspace.project, "/work/another-project");
            assert_eq!(
                shell.sessions[&cli].read(cx).input.read(cx).content,
                "Preserve CLI draft"
            );
        })
        .unwrap();
}

#[gpui::test]
fn elapsed_clock_is_local_bounded_and_stops_for_hidden_compact_and_finished_cards(
    cx: &mut TestAppContext,
) {
    let (window, a, _) = fixture(cx);
    busy(window, a, true, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // Drain the fixture's finite pane/reveal transitions before measuring
    // independent timer wakeups. No timeout or production cadence is changed.
    crate::clock::settle_animations(window, visual);
    let (activity, snapshots, transcript, transcript_passes) = window
        .update(visual, |shell, _, cx| {
            let transcript = shell.sessions[&a].read(cx).transcript.clone();
            (
                shell.live_agents.clone(),
                shell.live_agents.read(cx).snapshots,
                transcript.clone(),
                transcript.read(cx).render_passes,
            )
        })
        .unwrap();
    let ticks = activity.read_with(visual, |activity, _| activity.ticks);
    visual.executor().advance_clock(Duration::from_secs(1));
    visual.run_until_parked();
    assert_eq!(
        activity.read_with(visual, |activity, _| activity.ticks),
        ticks + 1
    );
    assert_eq!(
        transcript.read_with(visual, |transcript, _| transcript.render_passes),
        transcript_passes
    );
    // GPUI marks ancestor views dirty for any changed child. The expensive
    // session projection still belongs to semantic notifications, not redraws.
    assert_eq!(
        activity.read_with(visual, |activity, _| activity.snapshots),
        snapshots
    );
    for mode in [
        "hidden",
        "hidden-rail",
        "compact",
        "finished",
        "settings",
        "empty",
    ] {
        window
            .update(visual, |shell, w, cx| {
                shell.workspace.sidebar_activity_visible = mode != "hidden";
                shell.workspace.project_rail_open = mode != "compact" && mode != "hidden-rail";
                shell.workspace.preferences.collapsed_project_rail = if mode == "hidden-rail" {
                    "hidden"
                } else {
                    "compact"
                }
                .into();
                if mode == "finished" || mode == "empty" {
                    shell.sessions[&a].update(cx, |view, cx| {
                        view.busy = false;
                        cx.emit(SessionChanged);
                    });
                }
                if mode == "settings" {
                    shell.open_settings(crate::settings::Section::General, w, cx);
                }
                if mode == "empty" {
                    shell.close_settings(w, cx);
                    shell.unseen_finished.clear();
                }
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert!(
            activity.read_with(visual, |activity, _| activity.tick.is_none()),
            "{mode}"
        );
        let ticks = activity.read_with(visual, |activity, _| activity.ticks);
        visual.executor().advance_clock(Duration::from_secs(2));
        visual.run_until_parked();
        assert_eq!(
            activity.read_with(visual, |activity, _| activity.ticks),
            ticks,
            "{mode}"
        );
    }
}

#[gpui::test]
fn four_rows_expand_to_bounded_scrolling_and_preserve_footer_and_focus(cx: &mut TestAppContext) {
    let (window, a, b) = fixture(cx);
    let ids = window
        .update(cx, |shell, _, cx| {
            let mut ids = vec![a, b];
            for _ in 0..98 {
                let summary = locked(&shell.store)
                    .unwrap()
                    .create("codex", "/work/elsewhere")
                    .unwrap();
                ids.push(summary.id);
                shell.add_loaded(summary, cx);
            }
            shell
                .resources
                .update(cx, |resources, cx| resources.saturate(cx));
            ids
        })
        .unwrap();
    for &id in &ids {
        busy(window, id, true, cx);
    }
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1300.), px(900.)));
    visual.run_until_parked();
    for compact in [false, true] {
        window
            .update(visual, |shell, w, cx| {
                shell.workspace.project_rail_open = !compact;
                shell.live_agents.update(cx, |activity, _| {
                    activity.expanded = false;
                    activity
                        .scroll
                        .0
                        .borrow()
                        .base_handle
                        .set_offset(Point::default());
                });
                shell.sessions[&b].update(cx, |view, cx| {
                    view.input.update(cx, |input, cx| {
                        input.set_text("Do not lose this draft".into(), cx)
                    });
                });
                let input = shell.sessions[&b].read(cx).input.clone();
                w.focus(&input.focus_handle(cx), cx);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let shown = ids
            .iter()
            .filter(|id| bounds(visual, format!("live-agent-{id}")).is_some())
            .count();
        assert_eq!(shown, DEFAULT_ROWS);
        let focus = visual.update(|w, cx| w.focused(cx));
        let more = visual.debug_bounds("live-agents-more").unwrap();
        visual.simulate_click(more.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(
            visual.update(|w, cx| w.focused(cx)),
            focus,
            "pointer disclosure must retain composer focus"
        );
        window
            .update(visual, |shell, _, cx| {
                let activity = shell.live_agents.read(cx);
                assert!(activity.expanded);
                assert_eq!(activity.shown_count(), ids.len());
                assert_eq!(shell.workspace.active, Some(b));
                assert_eq!(
                    shell.sessions[&b].read(cx).input.read(cx).content,
                    "Do not lose this draft"
                );
            })
            .unwrap();
        window
            .update(visual, |shell, w, cx| {
                let focus = shell.live_agents.read(cx).list_focus.clone();
                w.focus(&focus, cx)
            })
            .unwrap();
        visual.simulate_keystrokes("end");
        visual.refresh().unwrap();
        visual.run_until_parked();
        let last = window
            .update(visual, |shell, _, cx| {
                shell.live_agents.read(cx).agents.last().unwrap().id
            })
            .unwrap();
        assert!(bounds(visual, format!("live-agent-{last}")).is_some());
        let mounted = ids
            .iter()
            .filter(|id| bounds(visual, format!("live-agent-{id}")).is_some())
            .count();
        assert!(
            mounted < 16,
            "virtual rows must be bounded by the viewport: {mounted}"
        );
        if !compact {
            let anchor = window
                .update(visual, |shell, _, cx| {
                    let activity = shell.live_agents.read(cx);
                    let offset = activity.scroll.0.borrow().base_handle.offset();
                    let top = (-f32::from(offset.y) / activity.row_height()).floor() as usize;
                    activity.agents[top].id
                })
                .unwrap();
            busy(window, a, false, visual);
            window
                .update(visual, |shell, _, cx| {
                    let activity = shell.live_agents.read(cx);
                    let offset = activity.scroll.0.borrow().base_handle.offset();
                    let top = (-f32::from(offset.y) / activity.row_height()).floor() as usize;
                    assert_eq!(
                        activity.agents[top].id, anchor,
                        "completion above the viewport retains its top session"
                    );
                })
                .unwrap();
            busy(window, a, true, visual);
        }
        // Short windows keep both the scrollable list and its disclosure above
        // the existing resource/subscription/settings footer, in either rail.
        visual.simulate_resize(gpui::size(px(1300.), px(520.)));
        visual.run_until_parked();
        let activity = visual.debug_bounds("sidebar-activity-card").unwrap();
        let subscriptions = visual.debug_bounds("subscriptions").unwrap();
        let resources = visual.debug_bounds("resource-pressure-chip").unwrap();
        let settings = visual.debug_bounds("classic-rail-settings").unwrap();
        assert!(activity.top() >= px(TITLE_BAR_HEIGHT));
        assert!(activity.bottom() <= resources.top());
        assert!(activity.bottom() <= subscriptions.top());
        assert!(subscriptions.bottom() <= settings.top());
        assert!(
            settings.bottom() <= px(520.),
            "settings must remain inside the window, compact={compact}: {settings:?}"
        );
        let more = visual.debug_bounds("live-agents-more").unwrap();
        assert!(more.bottom() <= activity.bottom());
        visual.simulate_click(more.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                let activity = shell.live_agents.read(cx);
                assert!(!activity.expanded);
                assert_eq!(activity.shown_count(), DEFAULT_ROWS);
                assert_eq!(
                    activity.scroll.0.borrow().base_handle.offset(),
                    Point::default()
                );
            })
            .unwrap();
        visual.simulate_resize(gpui::size(px(1300.), px(900.)));
        visual.run_until_parked();
    }
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
    cx.update(|cx| crate::clock::use_test_clock(cx));
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
        ] {
            if theme != "dark" && !matches!(name, "expanded" | "compact") {
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
                            view.activity = Some(
                                if view.summary.provider == "codex" {
                                    "Run cargo test"
                                } else {
                                    "Read src/main.rs"
                                }
                                .into(),
                            );
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
                        activity.expanded = expanded;
                        cx.notify();
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
                            activity.scroll.scroll_to_bottom();
                            cx.notify();
                        })
                    })
                    .unwrap();
                cx.run_until_parked();
            }
            crate::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("working-{theme}-{name}.png")))
                .unwrap();
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
fn historical_three_line_card_geometry_and_single_session_disclosure(cx: &mut TestAppContext) {
    let (window, a, _) = fixture(cx);
    busy(window, a, true, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1300.), px(1000.)));
    visual.run_until_parked();
    let header = visual.debug_bounds("live-agents-working-header").unwrap();
    let row = bounds(visual, format!("live-agent-{a}")).unwrap();
    let title = bounds(visual, format!("live-agent-{a}-title")).unwrap();
    let activity = bounds(visual, format!("live-agent-{a}-activity")).unwrap();
    let project = bounds(visual, format!("live-agent-{a}-project")).unwrap();
    let elapsed = bounds(visual, format!("live-agent-{a}-elapsed")).unwrap();
    assert_eq!(
        row.size.height,
        px(66.),
        "original 18/14/14px lines, two 4px gaps and 12px padding"
    );
    assert!(header.bottom() <= row.top());
    assert_eq!(activity.top() - title.bottom(), px(4.));
    assert_eq!(project.top() - activity.bottom(), px(4.));
    assert_eq!(project.top(), elapsed.top());
    assert_eq!(
        project.left(),
        title.left() + px(18.),
        "metadata provider icon remains below title"
    );
    assert!(visual.debug_bounds("live-agents-more").is_none());
    assert!(visual.debug_bounds("activity-next").is_none());
    window
        .update(visual, |shell, _, cx| {
            let activity = shell.live_agents.read(cx);
            assert_eq!(activity.agents.len(), 1);
            assert_eq!(activity.activity_label(&activity.agents[0]), "Working");
            assert_eq!(activity.elapsed(&activity.agents[0]).as_deref(), Some("1s"));
        })
        .unwrap();
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
    crate::clock::settle_animations(window, visual);
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
                assert_eq!(
                    activity.activity_label(&activity.agents[0]),
                    label.unwrap_or("Working")
                );
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
                "Run current command"
            );
        })
        .unwrap();
}
