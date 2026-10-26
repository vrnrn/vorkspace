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
fn paging_bounds_rows_and_preserves_footer_under_pressure_in_both_layouts(cx: &mut TestAppContext) {
    let (window, a, b) = fixture(cx);
    let ids = window
        .update(cx, |shell, _, cx| {
            let mut ids = vec![a, b];
            for _ in 0..7 {
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
    visual.simulate_resize(gpui::size(px(1300.), px(700.)));
    visual.run_until_parked();
    for compact in [false, true] {
        window
            .update(visual, |shell, _, cx| {
                shell.workspace.project_rail_open = !compact;
                shell
                    .live_agents
                    .update(cx, |activity, _| activity.page = 0);
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let activity = visual.debug_bounds("sidebar-activity-card").unwrap();
        let subscriptions = visual.debug_bounds("subscriptions").unwrap();
        let resources = visual.debug_bounds("resource-pressure-chip").unwrap();
        let settings = visual.debug_bounds("classic-rail-settings").unwrap();
        assert!(activity.bottom() <= subscriptions.top());
        assert!(subscriptions.bottom() <= settings.top());
        assert!(activity.top() >= px(TITLE_BAR_HEIGHT));
        assert!(if compact {
            activity.bottom() <= resources.top()
        } else {
            resources.bottom() <= activity.top()
        });
        let mut seen = HashSet::new();
        for page in 0..3 {
            let shown: Vec<_> = ids
                .iter()
                .copied()
                .filter(|id| bounds(visual, format!("live-agent-{id}")).is_some())
                .collect();
            assert_eq!(shown.len(), if page < 2 { PAGE_SIZE } else { 1 });
            for id in shown {
                assert!(seen.insert(id));
            }
            if page < 2 {
                let next = visual.debug_bounds("activity-next").unwrap();
                visual.simulate_click(next.center(), Default::default());
                visual.run_until_parked();
            }
        }
        assert_eq!(seen.len(), ids.len());
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
    let window = cx
        .open_window(gpui::size(px(1280.), px(800.)), |_, cx| {
            cx.new(|cx| Shell::new(store, summaries, "/work/vorkspace".into(), None, cx))
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(&mut cx, |shell, _, cx| {
            for id in shell.order.clone() {
                let view = shell.ensure_session(id, cx).unwrap();
                view.update(cx, |view, cx| {
                    view.busy = true;
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
            let id = shell.order[0];
            shell.sessions[&id].update(cx, |view, cx| view.sidebar_test_approval(true, cx));
            let id = shell.order[1];
            shell.sessions[&id].update(cx, |view, cx| {
                view.busy = false;
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    for theme in ["dark", "light", "harbor"] {
        for compact in [false, true] {
            window
                .update(&mut cx, |shell, _, cx| {
                    shell.workspace.preferences.theme =
                        if theme == "light" { "light" } else { "dark" }.into();
                    shell.workspace.preferences.workspace_theme = if theme == "harbor" {
                        "harbor"
                    } else {
                        "default"
                    }
                    .into();
                    shell.workspace.project_rail_open = !compact;
                    shell.invalidate_chrome(cx);
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            crate::clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!(
                    "activity-{theme}-{}.png",
                    if compact { "compact" } else { "expanded" }
                )))
                .unwrap();
        }
    }
}
