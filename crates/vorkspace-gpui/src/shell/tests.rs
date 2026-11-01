use super::*;
use gpui::{EntityInputHandler, TestAppContext, WindowHandle};
use vorkspace_core::store::Store;
#[gpui::test]
fn hovering_archive_preserves_history_and_does_not_select_the_row(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&a].update(cx, |session, _| {
                session.summary.cwd = "/work/vorkspace".into();
                session.summary.has_messages = true;
            });
            shell.sessions[&b].update(cx, |session, _| session.summary.has_messages = true);
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let row = visual.debug_bounds("sidebar-session-1").unwrap();
    visual.simulate_mouse_move(row.center(), None, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    let archive = visual
        .debug_bounds("archive-session-1")
        .expect("hover reveals Archive");
    visual.simulate_click(archive.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, window, cx| {
            assert_eq!(
                shell.workspace.active,
                Some(b),
                "the archive click must not select its row"
            );
            assert!(!shell.workspace.tabs.contains(&a));
            assert!(shell.sessions[&a].read(cx).summary.archived);
            assert!(!shell.sidebar_session_ids(cx).contains(&a));
            assert!(
                !locked(&shell.store)
                    .unwrap()
                    .load(a)
                    .unwrap()
                    .rows
                    .is_empty()
            );
            shell.run_menu(MenuTarget::SessionFilter, "archived", None, window, cx);
            assert!(shell.sidebar_session_ids(cx).contains(&a));
            shell.toggle_archive(a, cx);
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, window, cx| {
            assert!(!shell.sessions[&a].read(cx).summary.archived);
            shell.run_menu(MenuTarget::SessionFilter, "clear", None, window, cx);
            assert!(shell.sidebar_session_ids(cx).contains(&a));
            assert_eq!(shell.workspace.active, Some(b));
        })
        .unwrap();
}
#[gpui::test]
fn working_marks_animate_independently_and_completed_replies_clear_when_seen(
    cx: &mut TestAppContext,
) {
    let (window, a, b) = window(cx);
    let transcript = window
        .update(cx, |shell, _, cx| {
            shell.sessions[&a].update(cx, |session, cx| {
                session.busy = true;
                session.summary.cwd = "/work/vorkspace".into();
                session.summary.has_messages = true;
                cx.emit(SessionChanged);
            });
            shell.sessions[&b].read(cx).transcript.clone()
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    let before = transcript.read_with(cx, |view, _| view.render_passes);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(160));
    cx.run_until_parked();
    assert_eq!(
        before,
        transcript.read_with(cx, |view, _| view.render_passes)
    );
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&a].update(cx, |session, cx| {
                session.busy = false;
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, window, cx| {
            assert!(
                shell.unseen_finished.contains(&a),
                "a reply completed in the background"
            );
            shell.open(a, window, cx);
            assert!(
                !shell.unseen_finished.contains(&a),
                "opening it restores the provider mark"
            );
        })
        .unwrap();
}
#[gpui::test]
fn hover_preview_waits_and_never_materializes_a_conversation(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let (id, count) = window
        .update(cx, |shell, window, cx| {
            // Classic previews remain available without loading a conversation.
            shell.workspace.classic_layout = true;
            let mut summary = locked(&shell.store)
                .unwrap()
                .create("codex", "/work/vorkspace")
                .unwrap();
            summary.has_messages = true;
            let id = summary.id;
            shell.order.push(id);
            shell.session_summaries.insert(id, summary);
            let anchor = AnchorSlot::default();
            anchor.set(Some(Bounds::new(
                window.mouse_position() - gpui::point(px(5.), px(5.)),
                gpui::size(px(20.), px(20.)),
            )));
            let count = shell.sessions.len();
            shell.hover_session(id, anchor, true, true, window, cx);
            assert!(shell.session_preview.is_none());
            (id, count)
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(130));
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert_eq!(
                shell.session_preview.as_ref().map(|preview| preview.id),
                Some(id)
            );
            assert_eq!(shell.sessions.len(), count);
            assert!(!shell.sessions.contains_key(&id));
        })
        .unwrap();
}

#[gpui::test]
fn sidebar_rows_have_a_gap_and_preview_survives_crossing_into_the_card(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&a].update(cx, |session, _| {
                session.summary.cwd = "/work/vorkspace".into();
                session.summary.has_messages = true;
            });
            shell.sessions[&b].update(cx, |session, _| session.summary.has_messages = true);
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let first = visual
        .debug_bounds(Box::leak(format!("sidebar-session-{b}").into_boxed_str()))
        .unwrap();
    let second = visual
        .debug_bounds(Box::leak(format!("sidebar-session-{a}").into_boxed_str()))
        .unwrap();
    assert_eq!(first.size.height, px(32.));
    assert_eq!(
        second.top() - first.bottom(),
        px(1.),
        "rows retain their 1px gap"
    );
    let move_to = |position, cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_mouse_move(position, cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
    };
    move_to(first.origin + gpui::point(px(20.), px(16.)), cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(130));
    cx.run_until_parked();
    cx.refresh().unwrap();
    let (first, surface) = window
        .update(cx, |shell, _, _| {
            let preview = shell
                .session_preview
                .as_ref()
                .expect("hover opens a preview");
            assert_eq!(preview.id, b);
            (
                preview.anchor.get().unwrap(),
                preview.surface.get().unwrap(),
            )
        })
        .unwrap();
    // The close timer lets the pointer cross the 8px gap into the preview.
    move_to(
        gpui::point(first.right() + px(4.), first.top() + px(16.)),
        cx,
    );
    move_to(surface.center(), cx);
    window
        .update(cx, |shell, _, _| {
            assert!(shell.session_preview.is_some());
            assert!(
                shell.preview_close.is_none(),
                "entering the card cancels the close timer"
            );
        })
        .unwrap();
    move_to(second.origin + gpui::point(px(20.), px(16.)), cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(130));
    cx.run_until_parked();
    cx.refresh().unwrap();
    window
        .update(cx, |shell, window, cx| {
            let preview = shell.session_preview.as_ref().unwrap();
            assert_eq!(preview.id, a, "another row replaces the old preview");
            assert_eq!(preview.anchor.get().unwrap().top() - first.bottom(), px(1.));
            shell.open(a, window, cx);
            assert!(
                shell.session_preview.is_none(),
                "selection dismisses the card"
            );
        })
        .unwrap();
}
#[gpui::test]
fn high_usage_chip_opens_running_work_and_escape_closes_it(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    cx.update(crate::popover::bind);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("resource-pressure-chip").is_none());
    window
        .update(visual, |shell, _, cx| {
            shell
                .resources
                .update(cx, |monitor, cx| monitor.saturate(cx));
        })
        .unwrap();
    visual.run_until_parked();
    let chip = visual.debug_bounds("resource-pressure-chip").unwrap();
    let subscriptions = visual.debug_bounds("subscriptions").unwrap();
    let rail = visual.debug_bounds("classic-project-rail").unwrap();
    assert!(
        chip.bottom() <= subscriptions.top()
            && chip.left() >= rail.left()
            && chip.right() <= rail.right(),
        "resource pressure stays visible in the sidebar above subscriptions"
    );
    visual.simulate_click(chip.center(), Default::default());
    visual.run_until_parked();
    let dialog = visual.debug_bounds("resource-dialog").unwrap();
    assert_eq!(dialog.size.width, px(418.));
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("resource-dialog").is_none());
    assert!(visual.debug_bounds("resource-pressure-chip").is_some());
    window
        .update(visual, |shell, _, cx| shell.toggle_project_rail(cx))
        .unwrap();
    visual.run_until_parked();
    let chip = visual.debug_bounds("resource-pressure-chip").unwrap();
    let rail = visual.debug_bounds("classic-project-rail").unwrap();
    assert!(chip.left() >= rail.left() && chip.right() <= rail.right());
    visual.simulate_click(chip.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("resource-dialog").is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    window
        .update(visual, |shell, window, cx| {
            shell.open_settings(crate::settings::Section::General, window, cx);
        })
        .unwrap();
    visual.run_until_parked();
    let chip = visual.debug_bounds("resource-pressure-chip").unwrap();
    let rail = visual.debug_bounds("classic-project-rail").unwrap();
    assert!(
        chip.bottom() <= rail.bottom(),
        "settings keep the warning visible"
    );
}
#[gpui::test]
fn usage_footer_follows_active_tabs_and_provider_changes(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.usage_footer.read(cx).kind(), Kind::Claude);
            shell.open(a, w, cx);
            assert_eq!(shell.usage_footer.read(cx).kind(), Kind::Codex);
            shell.close(a, w, cx);
            assert_eq!(shell.workspace.project, "/work/another-project");
            assert_eq!(shell.workspace.active, None);
            shell.open(b, w, cx);
            assert_eq!(shell.usage_footer.read(cx).kind(), Kind::Claude);
            shell.sessions[&b].update(cx, |conversation, cx| {
                conversation.summary.provider = "codex".into();
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.usage_footer.read(cx).kind(), Kind::Codex);
        })
        .unwrap();
}
#[gpui::test]
fn context_menu_click_does_not_activate_the_tab_beneath_it(cx: &mut TestAppContext) {
    let (window, a, b) = same_project_window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open(a, w, cx);
            let anchor = Anchor::Point(gpui::point(px(600.), px(5.)));
            shell.open_menu(MenuTarget::Tab(b), anchor, w, cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.update_window(window.into(), |_, w, cx| {
        let position = gpui::point(px(612.), px(23.));
        w.simulate_mouse_move(position, cx);
        w.dispatch_event(
            gpui::PlatformInput::MouseDown(MouseDownEvent {
                position,
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        w.dispatch_event(
            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                position,
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
            }),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.tabs, [a]);
            assert_eq!(shell.workspace.active, Some(a));
            assert!(shell.menu.is_none());
        })
        .unwrap();
}
#[gpui::test]
fn tab_lifecycle_close_all_stays_in_the_selected_project(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open(a, w, cx);
            shell.run_menu(MenuTarget::Tab(a), "all", None, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.workspace.project, "/work/another-project");
            assert_eq!(shell.visible_tabs(cx).len(), 1);
            assert_eq!(shell.workspace.tabs.len(), 2);
            let active = shell.workspace.active.unwrap();
            assert!(![a, b].contains(&active));
            assert!(!shell.sessions[&active].read(cx).summary.has_messages);
            assert!(shell.sessions.contains_key(&a) && shell.sessions.contains_key(&b));
            assert!(shell.history.back.iter().all(|id| *id == b));
            assert!(shell.history.forward.is_empty());
            let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
            let saved: Workspace = serde_json::from_str(&saved).unwrap();
            assert_eq!(saved.project, shell.workspace.project);
            assert_eq!(saved.tabs, shell.workspace.tabs);
            assert_eq!(saved.active, shell.workspace.active);
        })
        .unwrap();
}
#[gpui::test]
fn tab_lifecycle_new_and_split_requests_do_not_undo_project_navigation(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    for split in [false, true] {
        window
            .update(cx, |shell, w, cx| {
                shell.open(b, w, cx);
                if split {
                    shell.split_session(SplitDir::Right, cx);
                } else {
                    shell.create(Kind::Codex, cx);
                }
                shell.select_project("/work/another-project".into(), w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                assert_eq!(shell.workspace.project, "/work/another-project");
                assert_eq!(shell.workspace.active, Some(a));
                assert_eq!(&shell.workspace.tabs[..2], &[a, b]);
                if split {
                    assert_eq!(shell.workspace.tabs.len(), 3);
                    let created = shell.workspace.tabs[2];
                    assert_eq!(shell.session_summaries[&created].cwd, "/work/vorkspace");
                    let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
                    let restored: Workspace = serde_json::from_str(&saved).unwrap();
                    assert!(restored.tabs.contains(&created));
                    shell.select_project("/work/vorkspace".into(), w, cx);
                    assert!(shell.visible_tabs(cx).contains(&TitleTab::Session(created)));
                } else {
                    assert_eq!(shell.workspace.tabs.len(), 2);
                }
                assert!(shell.workspace.layouts.is_empty());
            })
            .unwrap();
    }
}
#[gpui::test]
fn tab_lifecycle_closing_an_inactive_tab_keeps_notes_and_focus(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open_notes(w, cx);
            shell.close(a, w, cx);
            assert!(shell.notes.is_some());
            assert_eq!(shell.workspace.active, Some(b));
            assert_eq!(shell.workspace.project, "/work/vorkspace");
        })
        .unwrap();
}
#[gpui::test]
fn tab_lifecycle_closing_an_inactive_split_pane_keeps_notes(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            let summary = locked(&shell.store)
                .unwrap()
                .create("codex", "/work/another-project")
                .unwrap();
            let c = summary.id;
            shell.add(summary, cx);
            shell.open(a, w, cx);
            shell.workspace.split(c, SplitDir::Right);
            shell.open(b, w, cx);
            shell.open_notes(w, cx);
            shell.close_pane(a, w, cx);
            assert!(shell.notes.is_some());
            assert_eq!(shell.workspace.active, Some(b));
            assert_eq!(shell.workspace.tabs, [c, b]);
            assert_eq!(shell.workspace.project, "/work/vorkspace");
        })
        .unwrap();
}
#[gpui::test]
fn tab_lifecycle_pending_project_blanks_are_not_duplicated(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            for _ in 0..5 {
                shell.select_project("/work/empty".into(), w, cx);
            }
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.workspace.project, "/work/empty");
            assert_eq!(
                shell
                    .sessions
                    .values()
                    .filter(|s| s.read(cx).summary.cwd == "/work/empty")
                    .count(),
                1
            );
            assert!(shell.workspace.active.is_some());
        })
        .unwrap();
}
#[gpui::test]
fn tab_lifecycle_close_all_shortcut_keeps_the_project(cx: &mut TestAppContext) {
    let (window, a, _) = window(cx);
    cx.update(bind_keys);
    window
        .update(cx, |shell, w, cx| shell.open(a, w, cx))
        .unwrap();
    let modifier = "cmd";
    cx.simulate_keystrokes(window.into(), &format!("{modifier}-shift-w"));
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.workspace.project, "/work/another-project");
            assert_eq!(shell.visible_tabs(cx).len(), 1);
            assert_eq!(shell.workspace.tabs.len(), 2);
            let active = shell.workspace.active.unwrap();
            assert!(!shell.sessions[&active].read(cx).summary.has_messages);
        })
        .unwrap();
}
#[gpui::test]
fn tab_lifecycle_close_stays_in_project_and_keeps_running_drafts(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.sessions[&b].update(cx, |s, cx| {
                s.busy = true;
                s.input.update(cx, |i, cx| {
                    i.replace_text_in_range(None, "Keep this draft", w, cx)
                });
            });
            shell.close(b, w, cx);
            assert!(shell.focus.is_focused(w));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.workspace.project, "/work/vorkspace");
            let blank = shell.workspace.active.unwrap();
            assert_ne!(blank, b);
            assert_eq!(shell.workspace.tabs, [a, blank]);
            assert!(!shell.sessions[&blank].read(cx).summary.has_messages);
            assert!(shell.sessions[&b].read(cx).busy);
            shell.open(b, w, cx);
            assert_eq!(
                shell.sessions[&b].read(cx).input.read(cx).content.as_ref(),
                "Keep this draft"
            );
        })
        .unwrap();
}
#[gpui::test]
fn tab_lifecycle_opening_a_conversation_replaces_the_blank_tab(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.sessions[&b].update(cx, |s, _| s.summary.has_messages = true);
            shell.close(b, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            let blank = shell.workspace.active.unwrap();
            assert_eq!(shell.workspace.tabs, [a, blank]);
            shell.open(b, w, cx);
            assert_eq!(shell.workspace.tabs, [a, b]);
            assert_eq!(shell.workspace.active, Some(b));
        })
        .unwrap();
}
#[gpui::test]
fn tab_lifecycle_close_prefers_right_then_left_in_the_same_project(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            let mut extra = Vec::new();
            for _ in 0..2 {
                let summary = locked(&shell.store)
                    .unwrap()
                    .create("codex", "/work/vorkspace")
                    .unwrap();
                extra.push(summary.id);
                shell.add(summary, cx);
                shell.open(*extra.last().unwrap(), w, cx);
            }
            let (c, d) = (extra[0], extra[1]);
            shell.open(c, w, cx);
            shell.close(c, w, cx);
            assert_eq!(shell.workspace.active, Some(d));
            shell.close(b, w, cx);
            assert_eq!(shell.workspace.active, Some(d));
            let before = shell.workspace.tabs.clone();
            shell.close(c, w, cx);
            shell.run_menu(MenuTarget::Tab(c), "others", None, w, cx);
            shell.run_menu(MenuTarget::Tab(d), "unknown", None, w, cx);
            assert_eq!(shell.workspace.tabs, before);
            assert_eq!(shell.workspace.active, Some(d));
            shell.run_menu(MenuTarget::Tab(a), "others", None, w, cx);
            assert_eq!(shell.workspace.tabs, [a, d]);
            assert_eq!(shell.workspace.active, Some(d));
            assert_eq!(shell.workspace.project, "/work/vorkspace");
        })
        .unwrap();
}
#[gpui::test]
fn tab_lifecycle_project_switches_remember_the_last_focused_tab(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            let summary = locked(&shell.store)
                .unwrap()
                .create("codex", "/work/vorkspace")
                .unwrap();
            let c = summary.id;
            shell.add(summary, cx);
            shell.open(c, w, cx);
            shell.open(b, w, cx);
            shell.select_project("/work/another-project".into(), w, cx);
            assert_eq!(shell.workspace.active, Some(a));
            shell.select_project("/work/vorkspace".into(), w, cx);
            assert_eq!(shell.workspace.active, Some(b));
            let saved = serde_json::to_string(&shell.workspace).unwrap();
            let summaries: Vec<_> = shell
                .sessions
                .iter()
                .map(|(id, s)| (*id, s.read(cx).summary.cwd.clone()))
                .collect();
            let restored = Workspace::restore(Some(&saved), &summaries, "/work/vorkspace");
            assert_eq!(restored.project_focus["/work/vorkspace"], b);
            assert_eq!(restored.project_focus["/work/another-project"], a);
            assert_eq!(restored.active, Some(b));
        })
        .unwrap();
}
#[gpui::test]
fn tab_lifecycle_closing_a_file_only_tab_restores_the_project_chat(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open_review(
                crate::git_tools::review::ReviewTab::Commit {
                    cwd: "/work/vorkspace".into(),
                    sha: "abcdef1".into(),
                    short_sha: "abcdef1".into(),
                    subject: "Fix".into(),
                },
                w,
                cx,
            );
            let review = shell.reviews[&b].clone();
            review.update(cx, |r, cx| {
                r.state.chat_hidden = true;
                r.close_active(cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.workspace.project, "/work/vorkspace");
            assert!(shell.workspace.tabs.contains(&a));
            assert!(!shell.reviews.contains_key(&b));
            let active = shell.workspace.active.unwrap();
            assert_eq!(
                shell.sessions[&active].read(cx).summary.cwd,
                "/work/vorkspace"
            );
        })
        .unwrap();
}
#[gpui::test]
fn projects_without_tabs_land_on_one_reusable_blank_session(cx: &mut TestAppContext) {
    let (window, _, b) = window(cx);
    let blank = |shell: &Shell, cx: &App| {
        let id = shell
            .workspace
            .active
            .expect("a project always has an active session");
        let summary = &shell.sessions[&id].read(cx).summary;
        assert_eq!(summary.cwd, "/work/empty");
        assert!(!summary.has_messages);
        id
    };
    window
        .update(cx, |shell, w, cx| {
            shell.workspace.add_project("/work/empty");
            shell.select_project("/work/empty".into(), w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let (first, count) = window
        .update(cx, |shell, w, cx| {
            let id = blank(shell, cx);
            // Close its tab, leave, and come back: the same blank returns.
            shell.close(id, w, cx);
            shell.open(b, w, cx);
            shell.select_project("/work/empty".into(), w, cx);
            (id, shell.sessions.len())
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(blank(shell, cx), first);
            assert_eq!(shell.sessions.len(), count);
            assert!(shell.workspace.tabs.contains(&first));
        })
        .unwrap();
}
#[gpui::test]
fn header_menu_switches_projects_from_its_picker_flyout(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    cx.update(|cx| {
        input::bind(cx);
        crate::popover::bind(cx);
    });
    let before = window
        .update(cx, |shell, w, cx| {
            let anchor = Anchor::Slot(shell.project_anchor.clone());
            shell.open_menu(MenuTarget::Header, anchor, w, cx);
            shell.workspace.project.clone()
        })
        .unwrap();
    cx.run_until_parked();
    // Switch worktree is first; skip the separator to Switch project, open
    // its picker (which focuses the search field), and take the next project.
    cx.simulate_keystrokes(window.into(), "down right down enter");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(shell.menu.is_none());
            assert_ne!(shell.workspace.project, before);
        })
        .unwrap();
}
#[gpui::test]
fn project_appearance_menu_edits_and_background_dialog_persist_per_project(
    cx: &mut TestAppContext,
) {
    use vorkspace_core::project_appearance::{Background, COLORS, Effect, Scope};
    let (window, _, active) = window(cx);
    cx.update(|cx| {
        input::bind(cx);
        crate::popover::bind(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    window
        .update(cx, |shell, w, cx| {
            let anchor = Anchor::Slot(shell.project_anchor.clone());
            shell.open_menu(MenuTarget::Header, anchor, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    // The settings flyout opens beside the header menu and focuses its name.
    cx.simulate_keystrokes(window.into(), "down down right");
    cx.run_until_parked();
    cx.simulate_input(window.into(), "Small");
    cx.simulate_keystrokes(window.into(), "space");
    cx.simulate_input(window.into(), "Studio");
    cx.run_until_parked();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let flyout = window
        .update(visual, |shell, _, cx| {
            shell
                .menu
                .as_ref()
                .unwrap()
                .menu
                .read(cx)
                .submenu_region()
                .get()
                .unwrap()
        })
        .unwrap();
    let controls = visual.debug_bounds("project-appearance-controls").unwrap();
    let name = visual.debug_bounds("project-appearance-name").unwrap();
    let logo = visual.debug_bounds("choose-project-logo").unwrap();
    // TabGroupMenu's p-2 belongs to the whole card, including its actions.
    for inset in [
        controls.left() - flyout.left(),
        flyout.right() - controls.right(),
        name.top() - flyout.top(),
    ] {
        assert_eq!(inset, px(9.), "8px of padding inside the card's 1px frame");
    }
    assert_eq!(name.left(), controls.left());
    assert_eq!(logo.left() - controls.left(), px(2.));
    assert_eq!(logo.top() - name.bottom(), px(8.));
    assert_eq!(logo.size, gpui::size(px(36.), px(36.)));
    let background = visual.debug_bounds("menu-action-background").unwrap();
    assert_eq!(background.left() - flyout.left(), px(9.));
    assert_eq!(flyout.right() - background.right(), px(9.));
    let color = visual
        .debug_bounds("project-color-4")
        .expect("appearance controls in the flyout");
    visual.simulate_click(color.center(), Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let grid_slider = visual.debug_bounds("project-grid-slider").unwrap();
    let click_x = grid_slider.left() + grid_slider.size.width * (4. / 6.);
    visual.simulate_click(
        gpui::point(click_x, grid_slider.center().y),
        Default::default(),
    );
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, w, cx| {
            assert!(shell.menu.is_none());
            assert_eq!(shell.project_label("/work/vorkspace"), "Small Studio");
            let appearance = shell
                .workspace
                .project_appearance
                .get_mut("/work/vorkspace")
                .unwrap();
            assert_eq!(appearance.color, Some(COLORS[4]));
            assert_eq!(appearance.grid, Some(6));
            appearance.background = Some(Background {
                path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets/vorkspace.png").into(),
                ..Default::default()
            });
            assert!(
                !shell
                    .workspace
                    .project_appearance
                    .contains_key("/work/another-project")
            );
            shell.open_menu(
                MenuTarget::Project {
                    path: "/work/vorkspace".into(),
                    rail: false,
                },
                Anchor::Point(gpui::point(px(300.), px(60.))),
                w,
                cx,
            );
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let card = window
        .update(visual, |shell, _, cx| {
            let menu = shell.menu.as_ref().unwrap().menu.read(cx);
            menu.surface_region().get().unwrap()
        })
        .unwrap();
    let background_row = visual.debug_bounds("menu-action-background").unwrap();
    let last_row = visual.debug_bounds("menu-action-delete").unwrap();
    assert_eq!(background_row.left() - card.left(), px(9.));
    assert_eq!(card.right() - background_row.right(), px(9.));
    assert_eq!(background_row.size.height, px(32.));
    assert_eq!(card.bottom() - last_row.bottom(), px(9.));
    visual.simulate_click(background_row.center(), Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("project-background-dialog").is_some());
    let dialog = visual.debug_bounds("project-background-dialog").unwrap();
    let header = visual.debug_bounds("background-dialog-header").unwrap();
    let preview = visual.debug_bounds("background-dialog-preview").unwrap();
    let change = visual
        .debug_bounds("project-settings-Change image")
        .unwrap();
    let effect = visual.debug_bounds("background-effect-row").unwrap();
    let scope = visual.debug_bounds("background-scope-row").unwrap();
    let scope_control = visual.debug_bounds("background-scope-control").unwrap();
    let empty_row = visual.debug_bounds("background-empty-row").unwrap();
    let session_row = visual.debug_bounds("background-session-row").unwrap();
    let remove = visual
        .debug_bounds("project-settings-Remove background image")
        .unwrap();
    // Modal.tsx + ProjectBackgroundDialog.tsx: match the React box metrics.
    assert_eq!(dialog.size.width, px(418.));
    assert_eq!(preview.size, gpui::size(px(386.), px(162.)));
    assert_eq!(preview.left() - dialog.left(), px(16.));
    assert_eq!(preview.top() - header.bottom(), px(16.));
    assert_eq!(change.top() - preview.bottom(), px(8.));
    assert_eq!(change.size.height, px(32.));
    assert_eq!(effect.size.height, px(57.5));
    assert_eq!(scope_control.size, gpui::size(px(176.), px(32.)));
    for (before, after) in [
        (effect, scope),
        (scope, empty_row),
        (empty_row, session_row),
        (session_row, remove),
    ] {
        assert_eq!(after.top() - before.bottom(), px(20.));
    }
    assert_eq!(dialog.bottom() - remove.bottom(), px(16.));
    window
        .update(visual, |_, w, _| {
            assert_eq!(
                w.painted_quads()
                    .iter()
                    .filter(|q| q.background == content(0.15).into()
                        && q.bounds.size.width.0 == 172. * w.scale_factor()
                        && q.bounds.size.height.0 == 4. * w.scale_factor())
                    .count(),
                2
            );
        })
        .unwrap();
    let effect_trigger = visual.debug_bounds("project-background-effect").unwrap();
    assert_eq!(effect_trigger.size, gpui::size(px(144.), px(36.)));
    assert_eq!(effect_trigger.right(), preview.right());
    visual.simulate_click(effect_trigger.center(), Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("project-background-dialog").is_some());
    assert!(visual.debug_bounds("project-settings-Haze").is_none());
    visual.simulate_click(effect_trigger.center(), Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    for selector in ["project-settings-Haze", "project-settings-Empty only"] {
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
    }
    let opacity = visual.debug_bounds("empty-background-opacity").unwrap();
    visual.simulate_click(opacity.center(), Default::default());
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, _, _| {
            let background = shell.workspace.project_appearance["/work/vorkspace"]
                .background
                .as_ref()
                .unwrap();
            assert_eq!(background.effect, Effect::GradientBlur);
            assert_eq!(background.scope, Scope::Empty);
            assert!((background.empty_opacity - 0.36).abs() < 0.001);
            let restored = Workspace::restore(
                Some(&serde_json::to_string(&shell.workspace).unwrap()),
                &[(active, "/work/vorkspace".into())],
                "/work/vorkspace",
            );
            assert_eq!(
                restored.project_appearance,
                shell.workspace.project_appearance
            );
        })
        .unwrap();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, _, _| {
            assert!(shell.background_dialog.is_none())
        })
        .unwrap();
    assert!(visual.debug_bounds("project-chat-background").is_some());
    // Empty-only backgrounds disappear once a conversation has messages.
    window
        .update(visual, |shell, _, cx| {
            shell.sessions[&active].update(cx, |session, cx| {
                session.summary.has_messages = true;
                cx.notify();
            });
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    assert!(visual.debug_bounds("project-chat-background").is_none());
    visual.simulate_resize(gpui::size(px(820.), px(520.)));
    window
        .update(visual, |shell, w, cx| {
            shell.open_background_dialog("/work/vorkspace".into(), w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let compact = visual.debug_bounds("project-background-dialog").unwrap();
    let body = visual.debug_bounds("background-dialog-body").unwrap();
    assert!(compact.top() >= px(16.));
    assert!(compact.bottom() <= px(504.));
    assert!(body.bottom() <= compact.bottom());
}
#[gpui::test]
fn backgrounds_survive_tab_and_notes_switches_until_the_tab_closes(cx: &mut TestAppContext) {
    use vorkspace_core::project_appearance::{Background, Effect, ProjectAppearance};
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, _, cx| {
            for path in ["/work/another-project", "/work/vorkspace"] {
                shell.workspace.project_appearance.insert(
                    path.into(),
                    ProjectAppearance {
                        background: Some(Background {
                            path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets/vorkspace.png")
                                .into(),
                            effect: Effect::Dither,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                );
            }
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let background_b = window
        .update(cx, |shell, _, _| shell.backgrounds[&b].clone())
        .unwrap();
    let image_b = background_b.read_with(cx, |background, _| background.prepared_image().unwrap());
    window
        .update(cx, |shell, w, cx| shell.open(a, w, cx))
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let background_a = window
        .update(cx, |shell, _, _| shell.backgrounds[&a].clone())
        .unwrap();
    let image_a = background_a.read_with(cx, |background, _| background.prepared_image().unwrap());
    assert!(
        Arc::ptr_eq(&image_a, &image_b),
        "projects using the same artwork share its texture"
    );
    for _ in 0..3 {
        window
            .update(cx, |shell, w, cx| {
                shell.open(b, w, cx);
                shell.create(Kind::Codex, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                let new = shell.workspace.active.unwrap();
                assert_ne!(new, b);
                let prepared = shell.backgrounds[&new].read(cx).prepared_image().unwrap();
                assert!(
                    Arc::ptr_eq(&prepared, &image_b),
                    "new tabs must reuse the already decoded artwork"
                );
                assert!(prepared.size(0).width.0 > 0);
                shell.close(new, w, cx);
            })
            .unwrap();
        cx.run_until_parked();
    }
    for id in [b, a, b, a, b] {
        window
            .update(cx, |shell, w, cx| shell.open(id, w, cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                for (id, background, image) in
                    [(a, &background_a, &image_a), (b, &background_b, &image_b)]
                {
                    assert_eq!(
                        shell.backgrounds[&id].entity_id(),
                        background.entity_id(),
                        "hidden tabs retain the original background view"
                    );
                    let prepared = shell.backgrounds[&id].read(cx).prepared_image().unwrap();
                    assert!(
                        Arc::ptr_eq(&prepared, image),
                        "switching tabs must not regenerate the texture"
                    );
                    assert!(prepared.size(0).width.0 > 0);
                }
            })
            .unwrap();
    }
    window
        .update(cx, |shell, w, cx| shell.open_notes(w, cx))
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.backgrounds.len(), 2);
            shell.open(b, w, cx);
            shell.close(a, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(
                !shell.backgrounds.contains_key(&a),
                "closing a tab releases its background"
            );
            assert_eq!(shell.backgrounds[&b].entity_id(), background_b.entity_id());
            assert!(Arc::ptr_eq(
                &shell.backgrounds[&b].read(cx).prepared_image().unwrap(),
                &image_b
            ));
        })
        .unwrap();
}
#[gpui::test]
fn sidebar_shortcut_keeps_working_after_hiding_focused_search(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    cx.update(|cx| {
        input::bind(cx);
        bind_keys(cx);
    });
    window
        .update(cx, |shell, w, cx| shell.focus_search(w, cx))
        .unwrap();
    cx.refresh().unwrap();
    let m = "cmd";
    for visible in [false, true, false, true] {
        cx.simulate_keystrokes(window.into(), &format!("{m}-b"));
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.workspace.project_rail_open, visible)
            })
            .unwrap();
    }
}

#[gpui::test]
fn project_tools_shortcut_keeps_working_after_hiding_focused_history(cx: &mut TestAppContext) {
    use crate::workspace::SidebarTab;
    let (window, _, _) = window(cx);
    cx.update(|cx| {
        vorkspace_ui::clock::use_test_clock(cx);
        bind_keys(cx);
    });
    window
        .update(cx, |shell, w, cx| {
            shell.select_classic_sidebar(SidebarTab::Graph, w, cx);
            let history = shell.right_panel.as_ref().unwrap().read(cx).history.clone();
            w.focus(&history.focus_handle(cx), cx);
        })
        .unwrap();
    cx.refresh().unwrap();
    let modifier = "cmd";
    for visible in [false, true, false, true] {
        cx.simulate_keystrokes(window.into(), &format!("{modifier}-shift-b"));
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.workspace.sidebar_tab(), SidebarTab::Graph);
                assert_eq!(shell.workspace.sidebar_visible, visible);
                assert!(!shell.workspace.right_panel_open);
            })
            .unwrap();
    }
}
#[gpui::test]
fn removed_layout_shortcut_preserves_the_original_workspace_and_composer(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    cx.update(|cx| {
        vorkspace_ui::clock::use_test_clock(cx);
        bind_keys(cx);
        crate::input::bind(cx);
    });
    window
        .update(cx, |shell, w, cx| shell.focus_active(w, cx))
        .unwrap();
    cx.refresh().unwrap();
    let m = "cmd";
    cx.simulate_input(window.into(), "Keep this draft");
    cx.simulate_keystrokes(window.into(), &format!("{m}-shift-l"));
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.workspace.classic_layout);
            let session = shell.sessions[&shell.workspace.active.unwrap()].read(cx);
            assert_eq!(session.input.read(cx).content, "Keep this draft");
            assert!(!shell.workspace.right_panel_open);
        })
        .unwrap();
}

#[gpui::test]
fn title_tabs_draw_their_own_session_marks(cx: &mut TestAppContext) {
    let (window, a, _) = window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&a].update(cx, |session, cx| {
                session.busy = true;
                cx.emit(SessionChanged);
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            // One entity drawn in both the sidebar and the cached tab strip
            // only repaints along one path, freezing the tab's spinner.
            assert_ne!(
                shell.tab_marks[&a].entity_id(),
                shell.session_marks[&a].entity_id()
            );
            assert!(shell.tab_marks[&a].read(cx).state() == MarkState::Working);
        })
        .unwrap();
}
#[gpui::test]
fn resizing_project_rail_with_working_session_marks_keeps_cached_paint_valid(
    cx: &mut TestAppContext,
) {
    use std::time::Duration;
    let (window, a, b) = window(cx);
    cx.update(|cx| {
        bind_keys(cx);
        crate::input::bind(cx);
    });
    window
        .update(cx, |shell, w, cx| {
            for id in [a, b] {
                shell.sessions[&id].update(cx, |session, cx| {
                    session.summary.cwd = "/work/vorkspace".into();
                    session.summary.has_messages = true;
                    cx.emit(SessionChanged);
                });
            }
            shell.sessions[&a].update(cx, |session, cx| {
                session.busy = true;
                cx.emit(SessionChanged);
            });
            shell.focus_active(w, cx);
            shell.toggle_right_panel(w, cx);
            for _ in 0..30 {
                shell.workspace.tabs.extend([a, b]);
            }
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })
        .unwrap();
    };
    draw(cx);
    // Reusing the sidebar after an earlier sibling shrinks relocates its
    // text cache. A subsequent uncached slide must not replay old indices.
    window
        .update(cx, |shell, _, cx| {
            shell.workspace.tabs.truncate(2);
            shell.tabs_frame.update(cx, |_, cx| cx.notify());
            cx.notify();
        })
        .unwrap();
    draw(cx);
    window
        .update(cx, |shell, w, cx| shell.toggle_sidebar(w, cx))
        .unwrap();
    draw(cx);
    for step in 0..20 {
        for _ in 0..5 {
            cx.executor().advance_clock(Duration::from_millis(20));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx);
            })
            .unwrap();
        }
        window
            .update(cx, |shell, w, cx| {
                if step % 3 == 0 {
                    shell.toggle_sidebar(w, cx);
                }
                shell.toggle_project_rail(cx);
            })
            .unwrap();
        cx.run_until_parked();
    }
}

#[gpui::test]
fn titlebar_panel_buttons_keep_window_bounds_for_single_and_double_clicks(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    vorkspace_ui::clock::use_test_clock(cx);
    window
        .update(cx, |shell, w, cx| shell.focus_active(w, cx))
        .unwrap();
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(cx, |shell, window, cx| {
            let manager = shell.bot_manager();
            manager.update(cx, |manager, cx| {
                manager.create_named(Vec::new(), window, cx);
                manager.dismiss_settings(window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let original = window.update(cx, |_, w, _| w.window_bounds()).unwrap();
    for selector in ["toggle-sidebar", "toggle-right-panel"] {
        for click_count in [1, 2, 1, 2] {
            let position = cx.debug_bounds(selector).unwrap().center();
            cx.simulate_mouse_move(position, None, Default::default());
            cx.run_until_parked();
            let before = window
                .update(cx, |shell, _, _| {
                    (
                        shell.workspace.project_rail_open,
                        shell.workspace.right_panel_open,
                    )
                })
                .unwrap();
            cx.simulate_event(MouseDownEvent {
                position,
                button: MouseButton::Left,
                click_count,
                first_mouse: false,
                modifiers: Default::default(),
            });
            cx.simulate_event(gpui::MouseUpEvent {
                position,
                button: MouseButton::Left,
                click_count,
                modifiers: Default::default(),
            });
            cx.run_until_parked();
            window
                .update(cx, |shell, w, _| {
                    assert_eq!(w.window_bounds(), original);
                    if selector == "toggle-sidebar" {
                        assert_eq!(shell.workspace.project_rail_open, !before.0);
                    } else {
                        assert_eq!(shell.workspace.right_panel_open, !before.1);
                    }
                })
                .unwrap();
        }
    }
}

#[gpui::test]
fn left_tool_tabs_keep_file_review_and_shortcuts_live(cx: &mut TestAppContext) {
    use crate::workspace::SidebarTab;
    let (window, _, active) = window(cx);
    cx.update(|cx| {
        vorkspace_ui::clock::use_test_clock(cx);
        bind_keys(cx);
    });
    window
        .update(cx, |shell, w, cx| {
            shell.open_review(
                crate::git_tools::review::ReviewTab::File {
                    cwd: "/work/vorkspace".into(),
                    path: "README.md".into(),
                },
                w,
                cx,
            );
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1500.), px(900.)));
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    for (selector, header, expected) in [
        ("classic-tab-explorer", "explorer-header", SidebarTab::Files),
        ("classic-tab-changes", "changes-header", SidebarTab::Changes),
        (
            "classic-source-history",
            "history-toolbar",
            SidebarTab::Graph,
        ),
    ] {
        let tab = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(tab.center(), Default::default());
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
        let tool = visual.debug_bounds(header).unwrap();
        let review = visual.debug_bounds("review-tab-strip").unwrap();
        assert!(tool.right() <= sidebar.right());
        assert!(review.right() <= sidebar.left());
        assert!(visual.debug_bounds("right-panel-frame").is_none());
        window
            .update(visual, |shell, w, cx| {
                assert_eq!(shell.workspace.sidebar_tab(), expected);
                assert!(shell.workspace.sidebar_visible);
                assert!(shell.reviews.contains_key(&active));
                assert!(
                    shell
                        .right_panel
                        .as_ref()
                        .unwrap()
                        .focus_handle(cx)
                        .contains_focused(w, cx)
                );
            })
            .unwrap();
    }
    let modifier = "cmd";
    for visible in [false, true, false, true] {
        visual.simulate_keystrokes(&format!("{modifier}-shift-b"));
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.sidebar_tab(), SidebarTab::Graph);
                assert_eq!(shell.workspace.sidebar_visible, visible);
                assert!(shell.reviews.contains_key(&active));
            })
            .unwrap();
    }
}
#[gpui::test]
fn crowded_tabs_fit_and_hold_the_new_tab_button_still_while_one_opens(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(cx, |shell, _, cx| {
            shell.workspace.preferences.tab_animations = true;
            for _ in 0..12 {
                shell.create(Kind::Codex, cx);
            }
        })
        .unwrap();
    // Tab motion shares the executor clock with the window's frames.
    let settle = |cx: &mut gpui::VisualTestContext| {
        cx.executor().advance_clock(
            vorkspace_ui::tab_motion::DURATION + std::time::Duration::from_millis(20),
        );
        window
            .update(cx, |shell, _, cx| {
                shell.tabs_frame.update(cx, |_, cx| cx.notify())
            })
            .unwrap();
        cx.run_until_parked();
    };
    // Once to start the motions a render notices, once to finish them.
    settle(cx);
    settle(cx);
    let strip = cx.debug_bounds("title-tab-strip").unwrap();
    // Keep the strip's 6px gap before the trailing controls, or before
    // the open sidebar, whose header holds them instead.
    let first_control = cx
        .debug_bounds("sidebar-frame")
        .or_else(|| cx.debug_bounds("project-diff-stat"))
        .or_else(|| cx.debug_bounds("titlebar-file-search"))
        .unwrap();
    assert_eq!(strip.right(), first_control.left() - px(6.));
    let settled = cx.debug_bounds("new-tab").unwrap();
    assert!(settled.right() <= strip.right(), "every tab fits");
    let last = window
        .update(cx, |shell, _, cx| {
            shell.visible_tabs(cx).last().and_then(|t| t.session())
        })
        .unwrap()
        .unwrap();
    let last = cx
        .debug_bounds(Box::leak(format!("title-tab-{last}").into_boxed_str()))
        .unwrap();
    // The strip's 2px gap and the button's 2px margin, as when it has room.
    assert!(
        settled.left() - last.right() <= px(4.5),
        "a full strip keeps the usual gap before the new-tab button: {:?}",
        settled.left() - last.right()
    );
    let (selected, other) = window
        .update(cx, |shell, _, cx| {
            let tabs = shell.visible_tabs(cx);
            let selected = shell
                .active_title_tab(cx)
                .and_then(|t| t.session())
                .unwrap();
            let other = tabs
                .iter()
                .filter_map(|t| t.session())
                .find(|id| *id != selected);
            (selected, other.unwrap())
        })
        .unwrap();
    let bounds = |cx: &mut gpui::VisualTestContext, id: i64| {
        cx.debug_bounds(Box::leak(format!("title-tab-{id}").into_boxed_str()))
            .unwrap()
    };
    assert_eq!(
        bounds(cx, selected).size.width,
        px(SELECTED_TAB_MIN_WIDTH),
        "the selected tab shrinks only down to its floor"
    );
    assert!(bounds(cx, other).size.width < px(SELECTED_TAB_MIN_WIDTH));
    let close = |cx: &mut gpui::VisualTestContext, id: i64| {
        cx.debug_bounds(Box::leak(format!("close-title-tab-{id}").into_boxed_str()))
    };
    assert!(close(cx, selected).is_some(), "the selected tab can close");
    if bounds(cx, other).size.width < px(TAB_CLOSE_MIN_WIDTH) {
        assert!(close(cx, other).is_none(), "a collapsed tab only selects");
    }
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.tab_scroll.max_offset().x, px(0.), "nothing scrolls");
            shell.create(Kind::Codex, cx);
        })
        .unwrap();
    let mut seen = Vec::new();
    for _ in 0..16 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        window
            .update(cx, |shell, _, cx| {
                shell.tabs_frame.update(cx, |_, cx| cx.notify())
            })
            .unwrap();
        cx.run_until_parked();
        seen.push(cx.debug_bounds("new-tab").unwrap().left());
    }
    assert!(
        seen.iter().all(|x| (*x - settled.left()).abs() < px(0.5)),
        "the new-tab button holds still while a crowded strip makes room: {settled:?} {seen:?}"
    );
    window
        .update(cx, |shell, w, cx| {
            let first = shell.visible_tabs(cx)[0].session().unwrap();
            shell.close(first, w, cx);
        })
        .unwrap();
    let mut closing = Vec::new();
    for _ in 0..16 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        window
            .update(cx, |shell, _, cx| {
                shell.tabs_frame.update(cx, |_, cx| cx.notify())
            })
            .unwrap();
        cx.run_until_parked();
        closing.push(cx.debug_bounds("new-tab").unwrap().left());
    }
    assert!(
        closing
            .iter()
            .all(|x| (*x - settled.left()).abs() < px(0.5)),
        "and while a closed tab collapses: {closing:?}"
    );
}
#[gpui::test]
fn project_rail_toggle_holds_still_and_history_follows_the_rail(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    vorkspace_ui::clock::use_test_clock(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let toggle = visual.debug_bounds("toggle-sidebar").unwrap();
    assert_eq!(toggle.left(), px(traffic_light_inset(1., false)));
    let expanded = visual.debug_bounds("classic-project-rail").unwrap();
    assert_eq!(
        visual.debug_bounds("visit-forward").unwrap().right(),
        expanded.right() - px(6.)
    );
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("toggle-sidebar").unwrap(), toggle);
    assert_eq!(
        visual
            .debug_bounds("classic-project-rail")
            .unwrap()
            .size
            .width,
        px(48.)
    );
    assert_eq!(
        visual.debug_bounds("visit-back").unwrap().left(),
        toggle.right()
    );
    assert_eq!(
        visual.debug_bounds("visit-forward").unwrap().right(),
        px(traffic_light_inset(1., false) + 72.)
    );
    window
        .update(visual, |shell, _, _| {
            assert!(!shell.workspace.project_rail_open);
            assert!(shell.workspace.sidebar_visible);
        })
        .unwrap();
    window
        .update(visual, |_, window, _| window.toggle_fullscreen())
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |_, window, _| window.refresh())
        .unwrap();
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("toggle-sidebar").unwrap().left(),
        px(NO_TRAFFIC_LIGHTS)
    );
    assert_eq!(
        visual.debug_bounds("visit-forward").unwrap().right(),
        px(NO_TRAFFIC_LIGHTS + 72.)
    );
}
#[gpui::test]
fn compact_rail_workspace_tabs_reopen_the_sidebar_without_changing_the_project(
    cx: &mut TestAppContext,
) {
    use crate::workspace::SidebarTab;
    let (window, _, active) = window(cx);
    vorkspace_ui::clock::use_test_clock(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.toggle_project_rail(cx);
            shell.toggle_sidebar(window, cx);
            assert!(!shell.workspace.project_rail_open);
            assert!(!shell.workspace.sidebar_visible);
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    for (selector, expected) in [
        ("classic-compact-tab-explorer", SidebarTab::Files),
        ("classic-compact-tab-changes", SidebarTab::Changes),
        ("classic-compact-tab-sessions", SidebarTab::Sessions),
    ] {
        let button = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
        let rail = visual.debug_bounds("classic-project-rail").unwrap();
        let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
        assert_eq!(rail.size.width, px(48.));
        assert_eq!(
            sidebar.left(),
            visual.debug_bounds("main-pane-frame").unwrap().right()
        );
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.sidebar_tab(), expected);
                assert!(shell.workspace.sidebar_visible);
                assert!(!shell.workspace.project_rail_open);
                assert_eq!(shell.workspace.project, "/work/vorkspace");
                assert_eq!(shell.workspace.active, Some(active));
            })
            .unwrap();
    }
}
#[gpui::test]
fn original_layout_keeps_project_rail_sidebar_and_workspace_independent(cx: &mut TestAppContext) {
    vorkspace_ui::theme::configure(
        &vorkspace_ui::preferences::Preferences::default(),
        false,
        true,
    );
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, a, b) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1600.), px(1000.)));
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    visual
        .update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
    window
        .update(visual, |_, window, _| {
            let marker =
                gpui_apple::metal_renderer::backdrop_blur_marker(24. * window.scale_factor());
            assert!(
                window
                    .painted_quads()
                    .iter()
                    .any(|quad| quad.border_color == marker
                        && quad.corner_radii.top_left.0 == 7. * window.scale_factor()),
                "composer frosting keeps its blur and corner radius"
            );
        })
        .unwrap();
    let rail = visual.debug_bounds("classic-project-rail").unwrap();
    let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
    let main = visual.debug_bounds("main-pane-frame").unwrap();
    let tabs = visual.debug_bounds("title-tab-strip").unwrap();
    assert_eq!(rail.left(), px(0.));
    assert_eq!(rail.size.width, px(200.));
    assert_eq!(
        sidebar.left(),
        visual.debug_bounds("main-pane-frame").unwrap().right()
    );
    assert_eq!(sidebar.size.width, px(crate::right_panel::WIDTH_DEFAULT));
    assert_eq!(main.left(), rail.right());
    assert_eq!(sidebar.right(), px(1600.));
    assert_eq!(rail.top(), px(0.));
    assert_eq!(sidebar.top(), px(0.));
    assert_eq!(main.top(), px(0.));
    assert_eq!(rail.size.height, px(1000.));
    assert_eq!(sidebar.size.height, rail.size.height);
    assert_eq!(main.size.height, rail.size.height);
    assert!(tabs.left() >= main.left());
    assert!(visual.debug_bounds("right-panel-frame").is_none());
    for selector in [
        "classic-tab-sessions",
        "classic-tab-explorer",
        "classic-tab-changes",
    ] {
        assert!(visual.debug_bounds(selector).is_some());
    }
    let session_entity = window
        .update(visual, |shell, _, cx| {
            shell.sessions[&b].update(cx, |session, cx| {
                session.input.update(cx, |input, cx| {
                    input.set_text("Keep this original-layout draft".into(), cx)
                });
            });
            shell.workspace.split(a, SplitDir::Right);
            shell.workspace.active = Some(b);
            shell.invalidate_chrome(cx);
            shell.persist(cx);
            shell.sessions[&b].entity_id()
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    for rail_open in [false, true] {
        let toggle = visual.debug_bounds("toggle-sidebar").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        let rail = visual.debug_bounds("classic-project-rail").unwrap();
        assert_eq!(rail.size.width, px(if rail_open { 200. } else { 48. }));
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.workspace.project_rail_open, rail_open);
                assert!(shell.workspace.sidebar_visible);
                assert_eq!(shell.workspace.active, Some(b));
                assert_eq!(shell.workspace.layout(b).leaf_ids(), [b, a]);
                assert_eq!(shell.sessions[&b].entity_id(), session_entity);
                assert_eq!(
                    shell.sessions[&b].read(cx).input.read(cx).content,
                    "Keep this original-layout draft"
                );
            })
            .unwrap();
    }
    for visible in [false, true] {
        window
            .update(visual, |shell, window, cx| shell.toggle_sidebar(window, cx))
            .unwrap();
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        let rail = visual.debug_bounds("classic-project-rail").unwrap();
        let main = visual.debug_bounds("main-pane-frame").unwrap();
        assert_eq!(rail.size.width, px(200.));
        assert_eq!(main.left(), rail.right());
        assert_eq!(
            main.right(),
            px(1600.
                - if visible {
                    crate::right_panel::WIDTH_DEFAULT
                } else {
                    0.
                })
        );
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.workspace.sidebar_visible, visible);
                assert!(shell.workspace.project_rail_open);
                assert_eq!(shell.workspace.layout(b).leaf_ids(), [b, a]);
                assert_eq!(
                    shell.sessions[&b].read(cx).input.read(cx).content,
                    "Keep this original-layout draft"
                );
                let saved = shell.latest_state.lock().unwrap().clone();
                let summaries =
                    [a, b].map(|id| (id, shell.session_metadata(id, cx).summary.cwd.clone()));
                let restored = Workspace::restore(Some(&saved), &summaries, "/work/vorkspace");
                assert!(restored.classic_layout);
                assert_eq!(
                    restored.project_rail_open,
                    shell.workspace.project_rail_open
                );
                assert_eq!(restored.sidebar_visible, visible);
                assert_eq!(restored.layout(b).leaf_ids(), [b, a]);
            })
            .unwrap();
    }
}

#[gpui::test]
fn original_workspace_tabs_open_tools_on_the_left_and_remember_each_project(
    cx: &mut TestAppContext,
) {
    use crate::workspace::SidebarTab;
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, active) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1500.), px(900.)));
    visual.run_until_parked();
    for (selector, tab, header) in [
        ("classic-tab-explorer", SidebarTab::Files, "explorer-header"),
        ("classic-tab-changes", SidebarTab::Changes, "changes-header"),
        (
            "classic-source-history",
            SidebarTab::Graph,
            "history-toolbar",
        ),
        (
            "classic-source-changes",
            SidebarTab::Changes,
            "changes-header",
        ),
        (
            "classic-tab-sessions",
            SidebarTab::Sessions,
            "classic-session-filter",
        ),
    ] {
        let button = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
        let sidebar = visual.debug_bounds("sidebar-frame").unwrap();
        let header = visual.debug_bounds(header).unwrap();
        let main = visual.debug_bounds("main-pane-frame").unwrap();
        assert!(header.left() >= sidebar.left());
        assert!(header.right() <= sidebar.right());
        assert_eq!(sidebar.left(), main.right());
        assert!(visual.debug_bounds("right-panel-frame").is_none());
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.sidebar_tab(), tab);
                assert_eq!(shell.workspace.active, Some(active));
            })
            .unwrap();
    }
    window
        .update(visual, |shell, window, cx| {
            shell.select_classic_sidebar(SidebarTab::Changes, window, cx);
            shell.select_project("/work/another-project".into(), window, cx);
            assert_eq!(shell.workspace.sidebar_tab(), SidebarTab::Sessions);
            shell.select_classic_sidebar(SidebarTab::Files, window, cx);
            shell.select_project("/work/vorkspace".into(), window, cx);
            assert_eq!(shell.workspace.sidebar_tab(), SidebarTab::Changes);
            let saved = shell.latest_state.lock().unwrap().clone();
            let mut restored = Workspace::restore(Some(&saved), &[], "/work/vorkspace");
            assert_eq!(restored.sidebar_tab(), SidebarTab::Changes);
            restored.project = "/work/another-project".into();
            assert_eq!(restored.sidebar_tab(), SidebarTab::Files);
        })
        .unwrap();
}
#[gpui::test]
fn toggling_the_sidebar_after_its_cached_frames_does_not_replay_stale_paint(
    cx: &mut TestAppContext,
) {
    let (window, a, b) = window(cx);
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })
        .unwrap();
    };
    // The sidebar's search records its paint behind a long tab strip.
    window
        .update(cx, |shell, _, cx| {
            for _ in 0..30 {
                shell.workspace.tabs.extend([a, b]);
            }
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    draw(cx);
    // The strip shrinks while the sidebar replays its cached frame.
    window
        .update(cx, |shell, _, cx| {
            shell.workspace.tabs.truncate(2);
            shell.tabs_frame.update(cx, |_, cx| cx.notify());
            cx.notify();
        })
        .unwrap();
    draw(cx);
    // Sliding paints the sidebar uncached; its search must not replay
    // the indices it recorded two frames ago.
    window
        .update(cx, |shell, w, cx| shell.toggle_sidebar(w, cx))
        .unwrap();
    draw(cx);
    draw(cx);
}
#[gpui::test]
fn sidebar_transition_uses_executor_frames_and_retargets_without_jumping(cx: &mut TestAppContext) {
    use std::time::Duration;
    let (window, _, _) = window(cx);
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })
        .unwrap();
    };
    window
        .update(cx, |shell, w, cx| shell.toggle_sidebar(w, cx))
        .unwrap();
    draw(cx);
    window
        .update(cx, |shell, _, _| {
            assert!(shell.sidebar_slide.is_some());
            assert_eq!(shell.sidebar_fraction, 1.);
        })
        .unwrap();
    draw(cx);
    window
        .update(cx, |shell, _, _| assert_eq!(shell.sidebar_fraction, 1.))
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(70));
    draw(cx);
    let middle = window
        .update(cx, |shell, w, cx| {
            let middle = shell.sidebar_fraction;
            assert!(middle > 0. && middle < 1.);
            shell.toggle_sidebar(w, cx);
            middle
        })
        .unwrap();
    draw(cx);
    window
        .update(cx, |shell, _, _| {
            // Retargeting recomputes the spring around a new target.
            assert!((shell.sidebar_fraction - middle).abs() < f32::EPSILON);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_secs(1));
    draw(cx);
    window
        .update(cx, |shell, _, _| {
            assert!(shell.sidebar_slide.is_none());
            assert_eq!(shell.sidebar_fraction, 1.);
        })
        .unwrap();
}

#[gpui::test]
fn main_pane_covers_a_fixed_sidebar_with_a_clipped_fade(cx: &mut TestAppContext) {
    use std::time::Duration;
    let (window, _, active) = window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&active].update(cx, |session, _| {
                session.summary.has_messages = true;
            });
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    cx.update_window(window.into(), |_, w, cx| {
        w.simulate_next_frame(cx);
        w.draw(cx).clear(cx);
    })
    .unwrap();
    let row = window
        .update(cx, |shell, w, _| {
            w.painted_quads()
                .into_iter()
                .find(|q| {
                    q.background == content(0.1).into()
                        && q.bounds.size.height.0 == 32. * w.scale_factor()
                        && q.bounds.size.width.0
                            > shell.workspace.right_panel_width / 2. * w.scale_factor()
                        && q.bounds.left().0
                            >= (shell.viewport_width - shell.workspace.right_panel_width)
                                * w.scale_factor()
                })
                .expect("selected session has a painted background")
                .bounds
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let toggle = visual.debug_bounds("toggle-sidebar").unwrap();
    for visible in [false, true] {
        window
            .update(visual, |shell, w, cx| {
                shell.toggle_sidebar(w, cx);
                assert_eq!(shell.workspace.sidebar_visible, visible);
            })
            .unwrap();
        let mut previous = if visible { 0. } else { 1. };
        for ms in [30, 65, 110, 170] {
            window
                .update(visual, |shell, _, cx| {
                    shell
                        .sidebar_slide
                        .as_ref()
                        .unwrap()
                        .freeze_elapsed(Duration::from_millis(ms));
                    cx.notify();
                })
                .unwrap();
            visual
                .update_window(window.into(), |_, w, cx| {
                    w.simulate_next_frame(cx);
                    w.draw(cx).clear(cx);
                })
                .unwrap();
            let clip = visual.debug_bounds("sidebar-frame").unwrap();
            let content = visual.debug_bounds("sidebar-content").unwrap();
            let main = visual.debug_bounds("main-pane-frame").unwrap();
            assert_eq!(
                visual.debug_bounds("toggle-sidebar").unwrap().origin,
                toggle.origin
            );
            assert_eq!(content.right(), clip.right());
            assert!((clip.left() - main.right()).abs() < px(1.));
            window
                    .update(visual, |shell, w, _| {
                        let fraction = shell.sidebar_fraction;
                        assert!(if visible {
                            fraction > previous
                        } else {
                            fraction < previous
                        });
                        previous = fraction;
                        assert_eq!(content.size.width, px(shell.workspace.right_panel_width));
                        let quad = w.painted_quads().into_iter().find(|q| q.bounds == row);
                        if clip.left().as_f32() * w.scale_factor() >= row.right().0 {
                            assert!(quad.is_none(), "fully covered rows are culled");
                            return;
                        }
                        let quad = quad.expect("exposed sidebar row paints at its full size");
                        let alpha = quad.background.as_solid().unwrap().a;
                        assert!(
                            (alpha - 0.1 * fraction).abs() < 0.0001,
                            "painted alpha {alpha} follows {fraction}"
                        );
                        // The sidebar owns the 1px separator to its left;
                        // its contents clip at the separator's inner edge.
                        let clip_edge = (clip.left() + px(1.)).as_f32() * w.scale_factor();
                        assert!(
                            (quad.content_mask.bounds.left().0 - clip_edge).abs() < w.scale_factor(),
                            "row mask {:?} follows the inner sidebar clip edge {clip_edge} before main {main:?}",
                            quad.content_mask.bounds
                        );
                    })
                    .unwrap();
        }
        window
            .update(visual, |shell, _, cx| {
                shell
                    .sidebar_slide
                    .as_ref()
                    .unwrap()
                    .elapsed(Duration::from_secs(1));
                cx.notify();
            })
            .unwrap();
        visual
            .update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx);
            })
            .unwrap();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.sidebar_slide.is_none());
                assert_eq!(shell.sidebar_fraction, if visible { 1. } else { 0. });
            })
            .unwrap();
    }
    // Reopening the sidebar reverses its closing motion without jumping its edge.
    window
        .update(visual, |shell, w, cx| {
            shell.toggle_sidebar(w, cx);
            shell
                .sidebar_slide
                .as_ref()
                .unwrap()
                .elapsed(Duration::from_millis(65));
            let before = shell
                .sidebar_slide
                .as_ref()
                .unwrap()
                .at(vorkspace_ui::clock::Instant::now())
                .0;
            shell.toggle_sidebar(w, cx);
            assert!(shell.workspace.sidebar_visible);
            let after = shell
                .sidebar_slide
                .as_ref()
                .unwrap()
                .at(vorkspace_ui::clock::Instant::now())
                .0;
            assert!((after - before).abs() < 0.01);
        })
        .unwrap();
    visual.executor().advance_clock(Duration::from_secs(1));
    visual
        .update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })
        .unwrap();
    window
        .update(visual, |shell, _, _| {
            assert!(shell.sidebar_slide.is_none());
            assert_eq!(shell.sidebar_fraction, 1.);
        })
        .unwrap();
}
#[gpui::test]
fn switching_session_in_current_tab_keeps_its_slot_and_selection_settled(cx: &mut TestAppContext) {
    let (window, first, current) = same_project_window(cx);
    cx.update(|cx| {
        vorkspace_ui::clock::use_test_clock(cx);
        input::bind(cx);
        bind_keys(cx);
    });
    let next = window
        .update(cx, |shell, w, cx| {
            let mut summary = locked(&shell.store)
                .unwrap()
                .create("codex", &shell.workspace.project)
                .unwrap();
            summary.has_messages = true;
            summary.updated_at = 3;
            let next = summary.id;
            shell.add(summary, cx);
            for (id, updated_at) in [(first, 1), (current, 2)] {
                shell.sessions[&id].update(cx, |session, _| {
                    session.summary.has_messages = true;
                    session.summary.updated_at = updated_at;
                });
            }
            shell.open(current, w, cx);
            next
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let original_button = visual.debug_bounds("new-tab").unwrap();
    for (step, expected) in [(-1, next), (1, current), (-1, next), (1, current)] {
        window
            .update(visual, |shell, w, cx| {
                shell.workspace.preferences.tab_animations = true;
                shell.step_session(step, true, w, cx);
                assert_eq!(shell.workspace.active, Some(expected));
                let tabs = shell.visible_tabs(cx);
                assert_eq!(
                    tabs,
                    [TitleTab::Session(first), TitleTab::Session(expected)]
                );
                let (slots, moving) = shell.title_tab_motion.sync(
                    tabs.into_iter().map(|tab| (tab, ())).collect(),
                    true,
                    vorkspace_ui::clock::Instant::now(),
                );
                assert!(
                    !moving,
                    "replacing a session must not open or close a tab slot"
                );
                assert!(slots.iter().all(|slot| matches!(
                    slot,
                    vorkspace_ui::tab_motion::Slot::Live { open, .. } if *open == 1.
                )));
                assert_eq!(
                    shell.title_tab_selection.1,
                    Some(TitleTab::Session(expected))
                );
                assert!(
                    shell.title_tab_selection.0.is_none(),
                    "the selected slot did not change"
                );
            })
            .unwrap();
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("new-tab").unwrap(), original_button);
    }
    // Also exercise the actual shortcuts from the focused composer.
    let modifier = "cmd";
    for (key, expected) in [("up", next), ("down", current)] {
        visual.simulate_keystrokes(&format!("{modifier}-{key}"));
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert_eq!(shell.workspace.active, Some(expected));
                assert_eq!(shell.workspace.tabs.len(), 2);
            })
            .unwrap();
    }
}

#[gpui::test]
fn tab_project_and_session_keys_follow_vorkspace(cx: &mut TestAppContext) {
    let (window, a, b) = same_project_window(cx);
    cx.update(|cx| {
        input::bind(cx);
        bind_keys(cx);
    });
    let m = "cmd";
    let active = |cx: &mut TestAppContext| {
        window
            .update(cx, |shell, _, _| shell.workspace.active)
            .unwrap()
    };
    let keys = |keys: String, cx: &mut TestAppContext| {
        cx.simulate_keystrokes(window.into(), &keys);
        cx.run_until_parked();
    };
    window
        .update(cx, |shell, w, cx| shell.focus_active(w, cx))
        .unwrap();
    // Tabs are [a, b]: slots, the last slot, and wrapping next/previous.
    keys(format!("{m}-1"), cx);
    assert_eq!(active(cx), Some(a));
    keys(format!("{m}-9"), cx);
    assert_eq!(active(cx), Some(b));
    keys(format!("{m}-}}"), cx);
    assert_eq!(active(cx), Some(a));
    keys(format!("{m}-shift-["), cx);
    assert_eq!(active(cx), Some(b));
    keys("ctrl-tab".into(), cx);
    assert_eq!(active(cx), Some(a));

    // An empty composer hands Cmd+Shift+Right to project navigation...
    let project = |cx: &mut TestAppContext| {
        window
            .update(cx, |shell, _, _| shell.workspace.project.clone())
            .unwrap()
    };
    let before = project(cx);
    keys(format!("{m}-shift-right"), cx);
    let after = project(cx);
    assert_ne!(after, before);
    // ...but with a draft it selects text, as in any field.
    window
        .update(cx, |shell, w, cx| {
            let id = shell.workspace.active.unwrap();
            let input = shell.sessions[&id].read(cx).input.clone();
            w.focus(&input.focus_handle(cx), cx);
            input.update(cx, |i, cx| i.replace_text_in_range(None, "draft", w, cx));
        })
        .unwrap();
    keys(format!("{m}-shift-right"), cx);
    assert_eq!(project(cx), after);

    // Close the other tabs, keeping the active one.
    keys(format!("{m}-alt-t"), cx);
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(
                shell.visible_tabs(cx),
                [TitleTab::Session(shell.workspace.active_tab().unwrap())]
            );
        })
        .unwrap();
}
#[gpui::test]
fn session_menu_copies_ids_through_its_submenu(cx: &mut TestAppContext) {
    let (window, a, _) = window(cx);
    cx.update(crate::popover::bind);
    window
        .update(cx, |shell, w, cx| {
            let anchor = Anchor::Point(gpui::point(px(40.), px(120.)));
            shell.open_menu(MenuTarget::Session(a), anchor, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    // Pin → Rename → Copy session ID, open it, then skip the harness ID
    // (this session has none) for the vorkspace one.
    cx.simulate_keystrokes(window.into(), "down down right down enter");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| assert!(shell.menu.is_none()))
        .unwrap();
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(copied, Some(a.to_string()));
}
#[gpui::test]
fn desktop_database_sessions_load_and_copy_their_original_ids(cx: &mut TestAppContext) {
    let mut store = Store::open_vorkspace(std::path::Path::new(":memory:")).unwrap();
    let summary = store.create("codex", "/work/vorkspace").unwrap();
    let id = summary.id;
    let original = store.persisted_session_id(id).unwrap();
    store
        .write_rows(
            id,
            0,
            &[vorkspace_core::transcript::Row {
                role: "user".into(),
                text: "Existing desktop history".into(),
                first: true,
                sent_at: Some(123),
            }],
        )
        .unwrap();
    let summaries = store.sessions().unwrap();
    let mut state = Workspace::default();
    state.open(id);
    let saved = serde_json::to_string(&state).unwrap();
    let database = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                Shell::new(
                    database,
                    summaries,
                    "/work/vorkspace".into(),
                    Some(saved),
                    cx,
                )
            })
        })
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.update(crate::popover::bind);
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.workspace.active, Some(id));
            let transcript = shell.sessions[&id].read(cx).transcript.read(cx);
            assert_eq!(transcript.source_rows()[0].text, "Existing desktop history");
            shell.open_menu(
                MenuTarget::Session(id),
                Anchor::Point(gpui::point(px(40.), px(120.))),
                w,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "down down right down enter");
    cx.run_until_parked();
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(copied, Some(original));
}
#[gpui::test]
fn typing_in_the_session_filter_matches_titles_without_case_and_keeps_session_state(
    cx: &mut TestAppContext,
) {
    let (window, a, b) = same_project_window(cx);
    window
        .update(cx, |shell, _, cx| {
            for (id, title) in [(a, "Alpha retained work"), (b, "Beta current work")] {
                shell.sessions[&id].update(cx, |session, cx| {
                    session.summary.title = title.into();
                    session.summary.has_messages = true;
                    cx.emit(SessionChanged);
                });
            }
            shell.sessions[&b].update(cx, |session, cx| {
                session.input.update(cx, |input, cx| {
                    input.set_text("Keep the composer draft".into(), cx)
                })
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let identities = window
        .update(visual, |shell, _, _| {
            [
                shell.sessions[&a].entity_id(),
                shell.sessions[&b].entity_id(),
            ]
        })
        .unwrap();
    for (query, expected) in [
        ("aLpHa", vec![a]),
        ("bEtA", vec![b]),
        ("missing conversation", vec![]),
        ("", vec![b, a]),
    ] {
        window
            .update(visual, |shell, _, cx| {
                shell
                    .sidebar_search
                    .update(cx, |input, cx| input.set_text(query.into(), cx))
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                assert_eq!(shell.sidebar_session_ids(cx), expected, "query={query}");
                assert_eq!(shell.workspace.active, Some(b));
                assert_eq!(shell.sessions.len(), 2);
                assert_eq!(
                    [
                        shell.sessions[&a].entity_id(),
                        shell.sessions[&b].entity_id()
                    ],
                    identities
                );
                assert_eq!(
                    shell.sessions[&b].read(cx).input.read(cx).content,
                    "Keep the composer draft"
                );
            })
            .unwrap();
        for (id, selector) in [(a, "sidebar-session-1"), (b, "sidebar-session-2")] {
            assert_eq!(
                visual.debug_bounds(selector).is_some(),
                expected.contains(&id),
                "query={query}, row={id}"
            );
        }
    }
}
#[gpui::test]
fn sidebar_hover_updates_labels_without_rebuilding_the_chat(cx: &mut TestAppContext) {
    let (window, _, active) = window(cx);
    let root = window.root(cx).unwrap();
    let transcript = root.read_with(cx, |s, cx| s.sessions[&active].read(cx).transcript.clone());
    let before = transcript.read_with(cx, |t, _| t.render_passes);
    let inbox = gpui::VisualTestContext::from_window(window.into(), cx)
        .debug_bounds("sidebar-navigation-GitHub")
        .unwrap();
    cx.update_window(window.into(), |_, w, cx| {
        w.simulate_mouse_move(inbox.center(), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(root.read_with(cx, |s, _| s.sidebar_hover), Some("GitHub"));
    let notes = gpui::VisualTestContext::from_window(window.into(), cx)
        .debug_bounds("sidebar-navigation-Notes")
        .unwrap();
    cx.update_window(window.into(), |_, w, cx| {
        w.simulate_mouse_move(notes.center(), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(root.read_with(cx, |s, _| s.sidebar_hover), Some("Notes"));
    assert_eq!(transcript.read_with(cx, |t, _| t.render_passes), before);
}
#[gpui::test]
fn many_blank_tabs_fit_without_creating_sidebar_conversations(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, _, cx| {
            for _ in 0..18 {
                let summary = locked(&shell.store)
                    .unwrap()
                    .create("codex", "/work/vorkspace")
                    .unwrap();
                let id = summary.id;
                assert!(!summary.has_messages);
                shell.add(summary, cx);
                shell.workspace.open(id);
            }
            shell.invalidate_chrome(cx);
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            // The tabs share the strip instead of scrolling it.
            assert_eq!(
                shell.tab_scroll.max_offset().x,
                px(0.),
                "viewport {:?}, strip {:?}, children {}, first {:?}, last {:?}",
                w.viewport_size(),
                shell.tab_scroll.bounds(),
                shell.tab_scroll.children_count(),
                shell.tab_scroll.bounds_for_item(0),
                shell
                    .tab_scroll
                    .bounds_for_item(shell.workspace.tabs.len() - 1)
            );
            let id = shell.workspace.active.unwrap();
            let anchor = Anchor::Point(gpui::point(px(400.), px(30.)));
            shell.open_menu(MenuTarget::Tab(id), anchor, w, cx);
            // Closing every tab preserves one fresh blank tab, as in vorkspace.
            for id in shell.workspace.tabs.clone() {
                shell.close(id, w, cx);
            }
            assert_eq!(shell.workspace.tabs.len(), 1);
            assert!(!shell.tab_can_close(shell.workspace.tabs[0], cx));
        })
        .unwrap();
    cx.run_until_parked();
}
fn same_project_window(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64, i64) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&a].update(cx, |s, _| s.summary.cwd = "/work/vorkspace".into());
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    cx.run_until_parked();
    (window, a, b)
}
#[gpui::test]
fn pane_limit_rejects_a_fourth_session_and_preserves_the_source(cx: &mut TestAppContext) {
    let (window, a, b) = same_project_window(cx);
    cx.update(bind_keys);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1500.), px(900.)));
    window
        .update(visual, |shell, w, cx| {
            shell.open(b, w, cx);
            let c = locked(&shell.store)
                .unwrap()
                .create("codex", "/work/vorkspace")
                .unwrap();
            let c_id = c.id;
            shell.add(c, cx);
            shell.workspace.split(c_id, SplitDir::Down);
            let d = locked(&shell.store)
                .unwrap()
                .create("codex", "/work/vorkspace")
                .unwrap();
            let d_id = d.id;
            shell.add(d, cx);
            shell.workspace.split(d_id, SplitDir::Down);
            shell.workspace.open(a);
            shell.workspace.open(d_id);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            let before = shell.workspace.clone();
            let target = shell.workspace.active.unwrap();
            assert!(!shell.placement_allowed(DragSource::Session(a), target, Edge::Bottom));
            shell.pane_drag = Some(PaneDrag {
                source: DragSource::Session(a),
                start: gpui::point(px(0.), px(0.)),
                position: gpui::point(px(0.), px(0.)),
                active: true,
                over: Some((target, Edge::Bottom)),
                insert: None,
            });
            shell.finish_pane_drag(w, cx);
            assert_eq!(shell.workspace.tabs, before.tabs);
            assert_eq!(shell.workspace.layout(b), before.layout(b));
            assert_eq!(shell.workspace.active, before.active);
            assert_eq!(shell.error.as_deref(), Some(PANE_LIMIT_MESSAGE));
            assert!(
                shell
                    .app_placement(
                        target,
                        "/work/vorkspace",
                        &serde_json::json!({"placement":"down"}),
                        cx
                    )
                    .is_err()
            );
            shell.split_session(SplitDir::Down, cx);
            assert_eq!(shell.workspace.open_sessions(), before.open_sessions());
            shell.error = None;
            shell.focus_active(w, cx);
        })
        .unwrap();
    visual.simulate_keystrokes("cmd-shift-d");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.layout(b).max_session_stack(|id| id >= 0), 3);
            assert_eq!(shell.error.as_deref(), Some(PANE_LIMIT_MESSAGE));
        })
        .unwrap();
}

#[gpui::test]
fn pane_limit_async_creation_rechecks_space_before_publication(cx: &mut TestAppContext) {
    let (window, _, session) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open(session, w, cx);
            shell.viewport_width = 1800.;
            shell.pane_viewport.set(Some(Bounds::new(
                gpui::point(px(0.), px(0.)),
                gpui::size(px(1200.), px(800.)),
            )));
            let layout = shell.workspace.layout(session);
            assert!(shell.pane_layout_allowed(
                &layout,
                &layout.split_pane(session, SplitDir::Right, i64::MAX)
            ));
            let summary = locked(&shell.store)
                .unwrap()
                .create("codex", "/work/vorkspace")
                .unwrap();
            let revision = shell.navigation_revision;
            // Exercise the actual foreground continuation with a new size.
            shell.pane_viewport.set(Some(Bounds::new(
                gpui::point(px(0.), px(0.)),
                gpui::size(px(500.), px(800.)),
            )));
            shell.finish_split_creation(
                summary,
                None,
                (Some(session), revision, SplitDir::Right),
                w,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let new = shell.workspace.active.unwrap();
            assert_ne!(new, session);
            assert_eq!(shell.workspace.tab_of(new), Some(new));
            assert_eq!(shell.workspace.layout(session).leaf_ids(), [session]);
            assert_eq!(shell.error.as_deref(), Some(PANE_LIMIT_MESSAGE));
            assert!(shell.session_summaries.contains_key(&new));
        })
        .unwrap();
}

#[gpui::test]
fn pane_limit_rejected_chat_drop_keeps_attached_dirty_file(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;
    vorkspace_ui::clock::use_test_clock(cx);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("draft.txt");
    std::fs::write(&path, "original").unwrap();
    let (window, _, target) = window(cx);
    let source = window
        .update(cx, |shell, w, cx| {
            shell.open(target, w, cx);
            for _ in 0..2 {
                let summary = locked(&shell.store)
                    .unwrap()
                    .create("codex", "/work/vorkspace")
                    .unwrap();
                let id = summary.id;
                shell.add(summary, cx);
                shell.workspace.split(id, SplitDir::Down);
            }
            let summary = locked(&shell.store)
                .unwrap()
                .create("codex", "/work/vorkspace")
                .unwrap();
            let source = summary.id;
            shell.add(summary, cx);
            shell.workspace.split(source, SplitDir::Right);
            shell.open_review(
                crate::git_tools::review::ReviewTab::File {
                    cwd: directory.path().to_string_lossy().into_owned(),
                    path: path.to_string_lossy().into_owned(),
                },
                w,
                cx,
            );
            source
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, w, cx| {
            let review = shell.reviews[&source].clone();
            let editor = review.read(cx).active_editor().unwrap();
            let input = editor.read(cx).input.clone();
            input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "changed", w, cx)
            });
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, w, cx| {
            let review = shell.reviews[&source].clone();
            let editor = review.read(cx).active_editor().unwrap();
            assert!(editor.read(cx).dirty);
            let before = shell.workspace.layout(target);
            assert!(!shell.placement_allowed(DragSource::Pane(source), target, Edge::Bottom));
            shell.pane_drag = Some(PaneDrag {
                source: DragSource::Pane(source),
                start: gpui::point(px(0.), px(0.)),
                position: gpui::point(px(0.), px(0.)),
                active: true,
                over: Some((target, Edge::Bottom)),
                insert: None,
            });
            shell.finish_pane_drag(w, cx);
            assert_eq!(shell.workspace.layout(target), before);
            assert!(shell.workspace.review_owners.is_empty());
            assert_eq!(shell.reviews[&source], review);
            assert_eq!(review.read(cx).active_editor().unwrap(), editor);
            assert!(editor.read(cx).dirty);
        })
        .unwrap();
}

#[gpui::test]
fn pane_limit_overfull_layout_retains_and_reveals_all_panes(cx: &mut TestAppContext) {
    let (window, _, b) = window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1200.), px(800.)));
    let ids = window
        .update(visual, |shell, w, cx| {
            shell.open(b, w, cx);
            let mut ids = vec![b];
            for _ in 0..4 {
                let summary = locked(&shell.store)
                    .unwrap()
                    .create("codex", "/work/vorkspace")
                    .unwrap();
                let id = summary.id;
                shell.add(summary, cx);
                shell.workspace.split(id, SplitDir::Down);
                ids.push(id);
            }
            let saved = serde_json::to_string(&shell.workspace).unwrap();
            let sessions = shell
                .session_summaries
                .values()
                .map(|s| (s.id, s.cwd.clone()))
                .collect::<Vec<_>>();
            shell.workspace = Workspace::restore(Some(&saved), &sessions, "/work/vorkspace");
            cx.notify();
            ids
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("pane-scroll-down").is_some());
    assert!(visual.debug_bounds("pane-scroll-up").is_some());
    window
        .update(visual, |shell, w, cx| shell.open(ids[0], w, cx))
        .unwrap();
    visual.run_until_parked();
    let top_offset = window
        .update(visual, |shell, _, _| shell.pane_scroll.offset().y)
        .unwrap();
    let down = visual.debug_bounds("pane-scroll-down").unwrap().center();
    visual.simulate_click(down, Default::default());
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(shell.pane_scroll.offset().y < top_offset)
        })
        .unwrap();
    window
        .update(visual, |shell, w, cx| {
            shell.open(*ids.last().unwrap(), w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(shell.pane_scroll.offset().y < top_offset);
            assert_eq!(shell.workspace.layout(b).leaf_ids(), ids);
        })
        .unwrap();
}
#[gpui::test]
fn pane_limit_width_changes_with_window_and_preserves_horizontal_overflow(cx: &mut TestAppContext) {
    let (window, a, b) = same_project_window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(1800.), px(800.)));
    window
        .update(visual, |shell, w, cx| shell.open(b, w, cx))
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, _, _| {
            assert!(shell.pane_available_width() >= 840.);
            assert!(shell.placement_allowed(DragSource::Session(a), b, Edge::Right));
        })
        .unwrap();
    window
        .update(visual, |shell, _, cx| {
            shell.workspace.place(a, b, Edge::Right);
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    let before = window
        .update(visual, |shell, _, _| shell.workspace.layout(b))
        .unwrap();
    visual.simulate_resize(gpui::size(px(700.), px(800.)));
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
    let geometry = window
        .update(visual, |shell, _, _| {
            (
                shell.pane_viewport.get(),
                shell.pane_bounds.get(),
                shell.workspace.layout(b),
                shell.workspace.active,
            )
        })
        .unwrap();
    assert!(
        visual.debug_bounds("pane-scroll-left").is_some(),
        "{geometry:?}"
    );
    assert!(visual.debug_bounds("pane-scroll-right").is_some());
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.layout(b), before);
            assert_eq!(shell.workspace.layout(b).leaf_ids(), [b, a]);
            assert!(shell.pane_bounds.get().unwrap().size.width.as_f32() >= 840.);
            assert!(!shell.placement_allowed(DragSource::Session(i64::MAX), a, Edge::Right));
            assert!(shell.placement_allowed(DragSource::Session(i64::MAX), a, Edge::Bottom));
        })
        .unwrap();
}
#[test]
#[ignore = "Requires Metal and VORKSPACE_PANE_CAPTURE_DIR"]
fn pane_limit_native_capture() {
    let output = std::path::PathBuf::from(std::env::var_os("VORKSPACE_PANE_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let store = Arc::new(Mutex::new(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    let sessions: Vec<_> = (0..5)
            .map(|_| {
                let mut store = locked(&store).unwrap();
                let mut session = store.create("codex", "/work/pane-fixture").unwrap();
                store.write_rows(session.id, 0, &[
                    vorkspace_core::transcript::Row { role: "user".into(), text: "Please review this change and explain what happens when the window gets smaller.".into(), first: true, sent_at: None },
                    vorkspace_core::transcript::Row { role: "assistant".into(), text: "The panes retain their sessions. You can use the arrows below to reach any pane outside the visible area.".into(), first: true, sent_at: None },
                ]).unwrap();
                session.has_messages = true;
                session
            })
            .collect();
    let mut cx = gpui::HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    vorkspace_ui::clock::use_test_clock(&cx);
    for (name, dir, count, viewport) in [
        ("two-wide", SplitDir::Right, 2, (1500., 900.)),
        (
            "restored-horizontal-overflow",
            SplitDir::Right,
            5,
            (1200., 800.),
        ),
        (
            "restored-vertical-overflow",
            SplitDir::Down,
            5,
            (1200., 800.),
        ),
    ] {
        let mut state = Workspace::default();
        state.add_project("/work/pane-fixture");
        state.project = "/work/pane-fixture".into();
        state.open(sessions[0].id);
        for session in sessions.iter().take(count).skip(1) {
            state.split(session.id, dir);
        }
        let saved = serde_json::to_string(&state).unwrap();
        let store = store.clone();
        let summaries = sessions.clone();
        let window = cx
            .open_window(gpui::size(px(viewport.0), px(viewport.1)), |_, cx| {
                cx.new(|cx| {
                    Shell::new(
                        store,
                        summaries,
                        "/work/pane-fixture".into(),
                        Some(saved),
                        cx,
                    )
                })
            })
            .unwrap();
        cx.run_until_parked();
        vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("{name}.png")))
            .unwrap();
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}
#[gpui::test]
fn selected_file_reference_preserves_the_live_draft_and_requests_focus(cx: &mut TestAppContext) {
    let (window, _, active) = window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&active].update(cx, |chat, cx| {
                chat.input.update(cx, |input, cx| {
                    input.set_text("Existing draft\n".into(), cx)
                });
            });
            shell.add_to_chat(active, "@src/main.rs (lines 2-4)", cx);
            let chat = shell.sessions[&active].read(cx);
            assert_eq!(
                chat.input.read(cx).content,
                "Existing draft\n\n@src/main.rs (lines 2-4)\n\n"
            );
            assert_eq!(shell.pending_composer_focus, Some(active));
        })
        .unwrap();
}
pub(super) fn window(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64, i64) {
    window_with_scale(cx, 100)
}
pub(super) fn window_with_scale(
    cx: &mut TestAppContext,
    scale: u32,
) -> (WindowHandle<Shell>, i64, i64) {
    let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let mut a = store.create("codex", "/work/another-project").unwrap();
    let b = store.create("claude", "/work/vorkspace").unwrap();
    store
        .write_rows(
            a.id,
            0,
            &[vorkspace_core::transcript::Row {
                role: "assistant".into(),
                text: "History loaded when the adjacent tab becomes active".into(),
                first: true,
                sent_at: None,
            }],
        )
        .unwrap();
    a.has_messages = true;
    let ids = (a.id, b.id);
    let mut state = Workspace::default();
    state.preferences.ui_scale = scale;
    // Most layout tests were written for the sidebar at the right edge.
    state.preferences.sidebar_right = true;
    assert!(state.classic_layout);
    state.add_project("/work/another-project");
    state.add_project("/work/vorkspace");
    state.open(a.id);
    state.open(b.id);
    let saved = serde_json::to_string(&state).unwrap();
    let store = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut shell =
                    Shell::new(store, vec![b, a], "/work/vorkspace".into(), Some(saved), cx);
                // This shared fixture intentionally exposes both entities to
                // interaction tests; production restoration stays lazy.
                shell.ensure_session(ids.0, cx);
                shell
            })
        })
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    (window, ids.0, ids.1)
}
#[gpui::test]
fn tab_delete_confirms_and_targets_the_original_session(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window.update(cx, |shell, w, cx| {
            shell.open(a, w, cx);
            let (_, items, _, _) = shell.menu_items(&MenuTarget::Tab(a), &mut Vec::new(), w, cx);
            assert!(items.iter().any(|item| matches!(item, MenuItem::Action(action) if action.id == "delete" && !action.disabled && action.danger)));
            shell.run_menu(MenuTarget::Tab(a), "delete", None, w, cx);
            shell.open(b, w, cx);
        }).unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert!(shell.session_summaries.contains_key(&a));
            shell.open(a, w, cx);
            shell.run_menu(MenuTarget::Tab(a), "delete", None, w, cx);
            shell.open(b, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(!shell.session_summaries.contains_key(&a));
            assert!(shell.session_summaries.contains_key(&b));
            assert!(
                !locked(&shell.store)
                    .unwrap()
                    .sessions()
                    .unwrap()
                    .iter()
                    .any(|s| s.id == a)
            );
        })
        .unwrap();
}
#[gpui::test]
fn pane_surfaces_follow_the_edge_at_a_constant_width(cx: &mut TestAppContext) {
    use std::time::Duration;
    let (window, _, id) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open(id, w, cx);
            let manager = shell.bot_manager();
            manager.update(cx, |manager, cx| {
                manager.create_named(Vec::new(), w, cx);
                manager.dismiss_settings(w, cx);
            });
            shell.toggle_right_panel(w, cx);
        })
        .unwrap();
    for ms in [30, 65, 110, 170] {
        window
            .update(cx, |shell, _, cx| {
                shell
                    .right_slide
                    .as_ref()
                    .unwrap()
                    .freeze_elapsed(Duration::from_millis(ms));
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let clip = visual.debug_bounds("right-panel-frame").unwrap();
        let content = visual.debug_bounds("right-panel-content").unwrap();
        assert!((content.left() - clip.left()).abs() < px(2.));
        let full = window
            .update(visual, |shell, _, _| shell.workspace.right_panel_width)
            .unwrap();
        assert!((content.size.width - px(full - 1.)).abs() < px(1.));
        assert!(content.size.width > clip.size.width);
    }
    window
        .update(cx, |shell, w, cx| {
            shell
                .right_slide
                .as_ref()
                .unwrap()
                .elapsed(Duration::from_secs(1));
            shell.open(id, w, cx);
            shell.open_review(
                crate::git_tools::review::ReviewTab::Changes {
                    cwd: "/work/vorkspace".into(),
                    path: None,
                    staged: None,
                },
                w,
                cx,
            );
            shell
                .review_slide
                .as_ref()
                .unwrap()
                .1
                .freeze_elapsed(Duration::ZERO);
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    let mut settled_width = None;
    for ms in [30, 65, 110, 170] {
        window
            .update(cx, |shell, _, cx| {
                shell
                    .review_slide
                    .as_ref()
                    .unwrap()
                    .1
                    .freeze_elapsed(Duration::from_millis(ms));
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let clip = visual.debug_bounds("review-frame").unwrap();
        let content = visual.debug_bounds("review-content").unwrap();
        assert!((content.left() - clip.left()).abs() < px(2.));
        let expected = *settled_width.get_or_insert(content.size.width);
        assert!((content.size.width - expected).abs() < px(1.));
        assert!(content.size.width > clip.size.width);
    }
    window
        .update(cx, |shell, _, _| {
            let late = Duration::from_secs(2);
            if let Some(slide) = &shell.right_slide {
                slide.elapsed(late);
            }
            if let Some((_, slide, _)) = &shell.review_slide {
                slide.elapsed(late);
            }
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    window
        .update(cx, |shell, _, _| {
            assert!(shell.right_slide.is_none());
            assert!(shell.review_slide.is_none());
        })
        .unwrap();
}

#[gpui::test]
#[ignore = "Manual CoreText CPU frame-cost measurement; opens no native window"]
fn pane_motion_frame_cost_with_long_history(cx: &mut TestAppContext) {
    use std::time::Duration;
    use vorkspace_ui::clock::Instant;
    let cx = &mut TestAppContext::build_with_text_system(
        cx.dispatcher.clone(),
        Some("pane_motion_frame_cost"),
        Arc::new(gpui_apple::AppleTextSystem::new()),
    );
    let (window, _, id) = window(cx);
    let transcript = window.update(cx, |shell, w, cx| {
            shell.open(id, w, cx);
            let session = shell.sessions[&id].clone();
            session.update(cx, |session, cx| {
                session.summary.has_messages = true;
                let mut history = vorkspace_core::transcript::Transcript::default();
                for _ in 0..10_000 {
                    history.begin("assistant");
                    history.append("A conversation with enough text to wrap while its pane changes width.\nA second line of native text.");
                }
                session.transcript.update(cx, |view, cx| view.patch(0, history.rows, cx));
                cx.notify();
                session.transcript.clone()
            })
        }).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    let mut frames = Vec::new();
    let mut rendered_frames = 0;
    for opening in [false, true] {
        window
            .update(cx, |shell, w, cx| shell.toggle_sidebar(w, cx))
            .unwrap();
        for frame in 1..=40 {
            let root = window.root(cx).unwrap();
            let before = root.read_with(cx, |shell, _| shell.render_passes);
            let start = Instant::now();
            window
                .update(cx, |shell, window, cx| {
                    if let Some(slide) = &shell.sidebar_slide {
                        slide.freeze_elapsed(Duration::from_secs_f32(frame as f32 / 120.));
                    }
                    // Deliver animation invalidations before drawing. Doing
                    // this inside the explicit draw leaves queued notify
                    // effects that cause a second render after that draw.
                    window.simulate_next_frame(cx);
                    cx.notify();
                })
                .unwrap();
            if root.read_with(cx, |shell, _| shell.render_passes) == before {
                cx.update_window(window.into(), |_, window, cx| {
                    window.draw(cx).clear(cx);
                })
                .unwrap();
            }
            let rendered = root.read_with(cx, |shell, _| shell.render_passes) - before;
            // The workspace sidebar keeps the current composer focus.
            // Each controlled animation sample must draw exactly once,
            // including the frame that completes the transition.
            assert_eq!(rendered, 1, "opening={opening}, sample={frame}");
            rendered_frames += rendered;
            frames.push(start.elapsed().as_secs_f64() * 1000.);
            let rendered = transcript.read_with(cx, |view, _| view.rendered);
            assert!(
                rendered > 0 && rendered < 100,
                "expected visible history work, got {rendered}"
            );
        }
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.workspace.sidebar_visible, opening)
            })
            .unwrap();
    }
    frames.sort_by(f64::total_cmp);
    eprintln!(
        "CoreText, 10,000 messages, {} simulated 120Hz samples / {rendered_frames} rendered frames: CPU p50={:.2}ms p95={:.2}ms max={:.2}ms (workspace sidebar motion; excludes presentation/GPU)",
        frames.len(),
        frames[frames.len() / 2],
        frames[frames.len() * 95 / 100],
        frames.last().unwrap()
    );
}

#[gpui::test]
fn explorer_files_share_the_review_pane_and_dirty_close_can_cancel_or_save(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        vorkspace_ui::clock::use_test_clock(cx);
        crate::app_menu::init(cx);
    });
    use gpui::EntityInputHandler;
    let path = std::env::temp_dir().join(format!("vorkspace-file-pane-{}.rs", std::process::id()));
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let (window, _, id) = window(cx);
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open(id, w, cx);
            shell.open_review(
                crate::git_tools::review::ReviewTab::File {
                    cwd: "/tmp".into(),
                    path: path.to_string_lossy().into_owned(),
                },
                w,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, w, cx| {
            let pane = shell.reviews[&id].clone();
            pane.update(cx, |pane, cx| {
                pane.navigate_file(1, 4, cx);
                let editor = pane.active_editor().unwrap();
                let input = editor.read(cx).input.clone();
                input.update(cx, |i, cx| i.replace_text_in_range(None, " edited", w, cx));
            });
        })
        .unwrap();
    cx.run_until_parked();
    // Native menu-bar and status-menu Quit enter through the application
    // action. Cancelling must retain the same dirty editor and live window.
    cx.cx.update(|cx| cx.dispatch_action(&crate::Quit));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    // Restart also guards dirty files before tearing down the process.
    cx.cx.update(|cx| cx.dispatch_action(&crate::Restart));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    window
        .update(cx, |shell, window, cx| {
            shell.status_menu_action(status_menu::Action::Quit, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.unsaved_files(cx));
            shell.reviews[&id].update(cx, |pane, cx| pane.close_active(cx));
        })
        .unwrap();
    cx.refresh().unwrap();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.reviews[&id].read(cx).state.tabs.len(), 1);
            assert!(shell.unsaved_files(cx));
            shell.reviews[&id].update(cx, |pane, cx| pane.close_active(cx));
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    assert!(std::fs::read_to_string(&path).unwrap().contains(" edited"));
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.reviews.contains_key(&id));
            assert!(!shell.unsaved_files(cx));
        })
        .unwrap();
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn panes_split_resize_and_take_drops_from_panes_tabs_and_the_sidebar(cx: &mut TestAppContext) {
    use gpui::{Modifiers, point};
    let (window, a, b) = window(cx);
    cx.update(|cx| {
        bind_keys(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    window
        .update(cx, |shell, w, cx| {
            shell.sessions[&b].update(cx, |chat, _| chat.summary.has_messages = true);
            shell.open(b, w, cx);
            shell.focus_active(w, cx);
        })
        .unwrap();
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    let count_selector: &'static str =
        Box::leak(format!("title-tab-pane-count-{b}").into_boxed_str());
    let close_selector: &'static str = Box::leak(format!("close-title-tab-{b}").into_boxed_str());
    assert!(
        cx.debug_bounds(close_selector).is_some(),
        "a single pane can still be closed"
    );
    assert!(
        cx.debug_bounds(count_selector).is_none(),
        "a single pane has no count badge"
    );
    cx.simulate_keystrokes("cmd-d");
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    let new = window
        .update(cx, |shell, _, _| {
            let new = shell.workspace.active.unwrap();
            assert_eq!(shell.workspace.tabs, [a, b]);
            assert_eq!(shell.workspace.layout(b).leaf_ids(), [b, new]);
            new
        })
        .unwrap();
    window
        .update(cx, |shell, _, cx| {
            // Two sessions, one line each; the new pane has focus.
            let copy = shell.tab_copy(b, cx);
            let keys: Vec<_> = copy.panes.iter().map(|p| p.key).collect();
            assert_eq!(keys, [(b, false), (new, false)]);
            assert_eq!(copy.focus, 1);
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    assert!(
        cx.debug_bounds(count_selector).is_some(),
        "split tabs show their pane count"
    );
    let left = cx
        .debug_bounds(Box::leak(format!("pane-{b}").into_boxed_str()))
        .unwrap();
    let right = cx
        .debug_bounds(Box::leak(format!("pane-{new}").into_boxed_str()))
        .unwrap();
    assert!((left.right() - right.left()).abs() < px(1.));
    assert!((left.size.width - right.size.width).abs() < px(2.));

    // The sash between them drags the split.
    let sash = point(left.right(), left.center().y);
    cx.simulate_mouse_down(sash, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        sash - point(px(100.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_up(
        sash - point(px(100.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, _, _| {
            let leaves = shell.workspace.layout(b).leaves();
            assert!(leaves[0].rect.w < 0.45, "{leaves:?}");
        })
        .unwrap();

    // A sidebar session dropped on a pane's lower half stacks under it,
    // and the tab that held it closes.
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&a].update(cx, |s, _| {
                s.summary.cwd = "/work/vorkspace".into();
                s.summary.has_messages = true;
            });
            shell.invalidate_chrome(cx);
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    let row = cx
        .debug_bounds(Box::leak(format!("sidebar-session-{a}").into_boxed_str()))
        .unwrap()
        .center();
    let below = point(right.center().x, right.bottom() - px(20.));
    cx.simulate_mouse_down(row, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        row + point(px(20.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_move(below, MouseButton::Left, Modifiers::default());
    window
        .update(cx, |shell, _, _| {
            let drag = shell.pane_drag.as_ref().unwrap();
            assert_eq!(drag.over, Some((new, Edge::Bottom)));
        })
        .unwrap();
    cx.simulate_mouse_up(below, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.tabs, [b]);
            assert_eq!(shell.workspace.layout(b).leaf_ids(), [b, new, a]);
            assert_eq!(shell.workspace.active, Some(a));
            assert!(shell.pane_drag.is_none());
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);

    // Its header drags it up onto the title strip: a tab of its own.
    let grip = cx
        .debug_bounds(Box::leak(format!("drag-session-pane-{a}").into_boxed_str()))
        .unwrap()
        .center();
    let strip = cx
        .debug_bounds(Box::leak(format!("title-tab-{b}").into_boxed_str()))
        .unwrap();
    let before_b = point(strip.left() + px(10.), strip.center().y);
    cx.simulate_mouse_down(grip, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        grip + point(px(0.), px(-20.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_move(before_b, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(before_b, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.tabs, [a, b]);
            assert_eq!(shell.workspace.layout(b).leaf_ids(), [b, new]);
            assert_eq!(shell.workspace.active, Some(a));
        })
        .unwrap();

    // A tab press whose release the title bar swallowed stays inert:
    // dragging a pane below never drags that tab.
    let pressed = cx
        .debug_bounds(Box::leak(format!("title-tab-{b}").into_boxed_str()))
        .unwrap()
        .center();
    window
        .update(cx, |shell, _, cx| {
            let event = MouseDownEvent {
                position: pressed,
                button: MouseButton::Left,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            };
            shell.start_tab_drag(TitleTab::Session(b), &event, cx);
            assert!(shell.tab_drag.is_some());
        })
        .unwrap();
    cx.simulate_mouse_move(
        pressed + point(px(0.), px(300.)),
        None,
        Modifiers::default(),
    );
    window
        .update(cx, |shell, _, _| assert!(shell.tab_drag.is_none()))
        .unwrap();
    // A title tab pulled down onto a pane's left half moves in beside it.
    // This successful drop needs enough room for all three panes.
    cx.simulate_resize(gpui::size(px(2800.), px(900.)));
    window
        .update(cx, |shell, w, cx| shell.open(b, w, cx))
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    let tab = cx
        .debug_bounds(Box::leak(format!("title-tab-{a}").into_boxed_str()))
        .unwrap()
        .center();
    let pane = cx
        .debug_bounds(Box::leak(format!("pane-{b}").into_boxed_str()))
        .unwrap();
    let left_half = point(pane.left() + px(20.), pane.center().y);
    cx.simulate_mouse_down(tab, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        point(tab.x, px(TITLE_BAR_HEIGHT + PANE_DROP_PULL + 10.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    // Pulled out of the strip, sideways motion no longer slides the tab.
    cx.simulate_mouse_move(
        point(
            tab.x + px(300.),
            px(TITLE_BAR_HEIGHT + PANE_DROP_PULL + 10.),
        ),
        MouseButton::Left,
        Modifiers::default(),
    );
    window
        .update(cx, |shell, _, _| {
            let drag = shell.tab_drag.as_ref().unwrap();
            assert!(drag.armed);
            assert_eq!((drag.offset, drag.to), (px(0.), drag.from));
            assert!(shell.pane_drag.is_some());
        })
        .unwrap();
    cx.simulate_mouse_move(left_half, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(left_half, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.tabs, [b]);
            assert_eq!(shell.workspace.layout(b).leaf_ids(), [a, b, new]);
            assert_eq!(shell.workspace.active, Some(a));
        })
        .unwrap();

    // ⌘W closes the focused pane only; its neighbour takes focus.
    cx.simulate_keystrokes("cmd-w");
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.tabs, [b]);
            assert_eq!(shell.workspace.layout(b).leaf_ids(), [b, new]);
            assert_eq!(shell.workspace.active, Some(b));
        })
        .unwrap();
}
#[gpui::test]
fn a_split_pane_slides_in_from_its_edge_then_settles(cx: &mut TestAppContext) {
    let (window, _, b) = window(cx);
    cx.update(bind_keys);
    window
        .update(cx, |shell, w, cx| {
            shell.open(b, w, cx);
            shell.focus_active(w, cx);
        })
        .unwrap();
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-d");
    window
        .update(cx, |shell, _, _| {
            shell
                .pane_motion
                .as_ref()
                .unwrap()
                .slide
                .freeze_elapsed(std::time::Duration::from_millis(16));
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    let new = window
        .update(cx, |shell, _, _| {
            let new = shell.workspace.active.unwrap();
            let motion = shell.pane_motion.as_ref().expect("the split animates");
            assert_eq!(motion.entering.get(&new), Some(&Edge::Right));
            assert_eq!(motion.from[&b].w, 1.);
            new
        })
        .unwrap();
    let entering = cx
        .debug_bounds(Box::leak(format!("pane-{new}").into_boxed_str()))
        .unwrap();
    let pane = cx
        .debug_bounds(Box::leak(format!("pane-{b}").into_boxed_str()))
        .unwrap();
    // Mid-slide: the chat still holds most of the width.
    assert!(
        entering.size.width < pane.size.width,
        "{entering:?} {pane:?}"
    );
    window
        .update(cx, |shell, _, cx| {
            shell
                .pane_motion
                .as_ref()
                .unwrap()
                .slide
                .elapsed(std::time::Duration::from_secs(1));
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| assert!(shell.pane_motion.is_none()))
        .unwrap();
    let entering = cx
        .debug_bounds(Box::leak(format!("pane-{new}").into_boxed_str()))
        .unwrap();
    let pane = cx
        .debug_bounds(Box::leak(format!("pane-{b}").into_boxed_str()))
        .unwrap();
    assert!((entering.size.width - pane.size.width).abs() < px(2.));
}
#[gpui::test]
fn a_closed_pane_slides_out_to_its_edge_while_its_neighbour_grows(cx: &mut TestAppContext) {
    let (window, _, b) = window(cx);
    cx.update(|cx| {
        bind_keys(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    window
        .update(cx, |shell, w, cx| {
            shell.open(b, w, cx);
            shell.focus_active(w, cx);
        })
        .unwrap();
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-d");
    cx.run_until_parked();
    vorkspace_ui::clock::use_test_clock(cx);
    let new = window
        .update(cx, |shell, w, cx| {
            let new = shell.workspace.active.unwrap();
            shell.close_pane(new, w, cx);
            let motion = shell.pane_motion.as_ref().expect("closing animates");
            assert_eq!(motion.header, Some(false));
            assert_eq!(motion.leaving.len(), 1);
            assert_eq!(
                (motion.leaving[0].0.id, motion.leaving[0].1),
                (new, Edge::Right)
            );
            assert_eq!(shell.workspace.layout(b).leaf_ids(), [b]);
            new
        })
        .unwrap();
    cx.refresh().unwrap();
    assert!(
        cx.debug_bounds(Box::leak(format!("split-header-{new}").into_boxed_str()))
            .is_some(),
        "still painted while it slides out"
    );
    window
        .update(cx, |shell, _, _| {
            // Midway it has slid half its width toward the right edge,
            // and the lone pane has grown over the same distance.
            let motion = shell.pane_motion.as_ref().unwrap();
            let (leaf, _, to) = motion.leaving[0];
            let mid = leaf.rect.lerp(to, 0.5);
            let grown = motion.from[&b].lerp(Rect::FULL, 0.5);
            assert!((mid.x - (grown.x + grown.w)).abs() < 1e-5);
            assert!((mid.w - 0.25).abs() < 1e-5);
        })
        .unwrap();
    window
        .update(cx, |shell, _, cx| {
            let motion = shell.pane_motion.as_ref().unwrap();
            motion.slide.elapsed(std::time::Duration::from_secs(1));
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| assert!(shell.pane_motion.is_none()))
        .unwrap();
    assert!(
        cx.debug_bounds(Box::leak(format!("pane-{new}").into_boxed_str()))
            .is_none()
    );
}
#[gpui::test]
fn closing_session_header_stays_hidden_on_the_settling_frame(cx: &mut TestAppContext) {
    assert_closed_header_stays_hidden(cx, false);
}

#[gpui::test]
fn closing_file_header_stays_hidden_on_the_settling_frame(cx: &mut TestAppContext) {
    assert_closed_header_stays_hidden(cx, true);
}

fn assert_closed_header_stays_hidden(cx: &mut TestAppContext, file: bool) {
    use std::time::Duration;
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, session, other) = same_project_window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.open(session, w, cx);
            if file {
                shell.open_review(
                    crate::git_tools::review::ReviewTab::File {
                        cwd: "/work/vorkspace".into(),
                        path: "/tmp/header-close.rs".into(),
                    },
                    w,
                    cx,
                );
            } else {
                shell.workspace.place(other, session, Edge::Right);
                shell.open(session, w, cx);
            }
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let header = Box::leak(format!("split-header-{session}").into_boxed_str());
    assert!(visual.debug_bounds(header).is_some());
    vorkspace_ui::clock::use_test_clock(cx);
    window
        .update(visual, |shell, w, cx| {
            if file {
                shell.close_review(session, cx);
            } else {
                shell.close_pane(other, w, cx);
            }
        })
        .unwrap();

    // Inspect the exact frame that consumes the finished animation. A
    // later refresh would hide a one-frame return of the full header.
    visual
        .update_window(window.into(), |_, w, cx| {
            let root = w.root::<Shell>().flatten().unwrap();
            root.update(cx, |shell, _| {
                if file {
                    shell
                        .review_slide
                        .as_ref()
                        .unwrap()
                        .1
                        .elapsed(Duration::from_secs(1));
                } else {
                    shell
                        .pane_motion
                        .as_ref()
                        .unwrap()
                        .slide
                        .elapsed(Duration::from_secs(1));
                }
            });
            w.refresh();
            w.draw(cx).clear(cx);
        })
        .unwrap();
    assert!(
        visual.debug_bounds(header).is_none(),
        "the settled frame must already omit the disappearing header"
    );
    let transcript = window
        .update(visual, |shell, _, cx| {
            shell.sessions[&session].read(cx).transcript.clone()
        })
        .unwrap();
    let settled = transcript.read_with(visual, |t, _| t.list.viewport_bounds());
    visual
        .update_window(window.into(), |_, w, cx| {
            w.refresh();
            w.draw(cx).clear(cx);
        })
        .unwrap();
    assert!(visual.debug_bounds(header).is_none());
    assert_eq!(
        transcript.read_with(visual, |t, _| t.list.viewport_bounds()),
        settled,
        "the transcript must not jump after the closing animation settles"
    );
}

#[gpui::test]
fn a_crowding_pane_collapses_the_sidebar_and_its_room_brings_it_back(cx: &mut TestAppContext) {
    let (window, _, b) = window(cx);
    cx.update(|cx| {
        bind_keys(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    cx.simulate_window_resize(window.into(), gpui::size(px(1200.), px(800.)));
    window
        .update(cx, |shell, w, cx| {
            shell.open(b, w, cx);
            shell.focus_active(w, cx);
        })
        .unwrap();
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(shell.workspace.sidebar_visible);
            assert_eq!(shell.viewport_width, 1200.);
        })
        .unwrap();
    cx.simulate_keystrokes("cmd-d");
    cx.run_until_parked();
    let new = window
        .update(cx, |shell, _, cx| {
            // A 200px project rail and 260px sidebar leave ~370px per pane.
            assert!(!shell.workspace.sidebar_visible);
            assert!(shell.narrowest_chat(false, cx) >= MIN_CHAT_WIDTH);
            shell.workspace.active.unwrap()
        })
        .unwrap();
    window
        .update(cx, |shell, w, cx| shell.close_pane(new, w, cx))
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| assert!(shell.workspace.sidebar_visible))
        .unwrap();
    // A sidebar the user hid stays hidden when room comes back.
    cx.simulate_keystrokes("cmd-d");
    cx.run_until_parked();
    // Collapsed for the split; shown, then hidden again by hand.
    cx.simulate_keystrokes("cmd-shift-b cmd-shift-b");
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert!(!shell.workspace.sidebar_visible);
            let pane = shell.workspace.active.unwrap();
            shell.close_pane(pane, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| assert!(!shell.workspace.sidebar_visible))
        .unwrap();
}
#[gpui::test]
fn reordered_tabs_slide_into_place_instead_of_jumping(cx: &mut TestAppContext) {
    use gpui::{Modifiers, point};
    let (window, a, b) = same_project_window(cx);
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    let first = cx
        .debug_bounds(Box::leak(format!("title-tab-{a}").into_boxed_str()))
        .unwrap();
    let second = cx
        .debug_bounds(Box::leak(format!("title-tab-{b}").into_boxed_str()))
        .unwrap();
    let slot = (second.left() - first.left()).as_f32();
    let grab = first.center();
    cx.simulate_mouse_down(grab, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        grab + point(px(slot * 0.8), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            // The neighbour makes room by easing toward the vacated slot.
            let shift = shell.tab_shifts[&TitleTab::Session(b)];
            assert_eq!(shift.to, -slot);
            assert!(shift.at(shift.start).0 == 0.);
            let (later, _) = shift.at(shift.start + std::time::Duration::from_millis(80));
            assert!(later < 0. && later > -slot);
        })
        .unwrap();
    cx.simulate_mouse_up(
        grab + point(px(slot * 0.8), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.tabs, [b, a]);
            // Released short of its slot, the tab glides the rest of
            // the way rather than snapping there.
            let shift = shell.tab_shifts[&TitleTab::Session(a)];
            assert!((shift.from - (-slot * 0.2)).abs() < 2., "{}", shift.from);
            assert_eq!(shift.to, 0.);
            let end = shift.start + std::time::Duration::from_millis(200);
            assert_eq!(shift.at(end), (0., true));
        })
        .unwrap();
}
#[gpui::test]
fn the_terminal_dock_toggles_adds_closes_and_moves(cx: &mut TestAppContext) {
    let (window, _, b) = window(cx);
    cx.update(|cx| {
        bind_keys(cx);
        crate::terminal::bind(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    window
        .update(cx, |shell, w, cx| {
            shell.open(b, w, cx);
            shell.focus_active(w, cx);
        })
        .unwrap();
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    assert!(cx.debug_bounds("terminal-dock").is_none());
    cx.simulate_keystrokes("cmd-j");
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    let project = window
        .update(cx, |shell, w, cx| {
            let project = shell.workspace.project.clone();
            let dock = &shell.workspace.docks[&project];
            assert!(dock.open);
            assert_eq!(dock.terminals.as_slice(), std::slice::from_ref(&project));
            assert_eq!(shell.terminals[&project].len(), 1);
            // The new terminal has the keys.
            assert_eq!(shell.dock_focused(w, cx), Some(0));
            project
        })
        .unwrap();
    let dock = cx.debug_bounds("terminal-dock").unwrap();
    assert!((dock.size.height - px(220.)).abs() < px(2.));
    // ⌘T in the dock opens another terminal, not a session tab.
    let tabs = window.update(cx, |s, _, _| s.workspace.tabs.len()).unwrap();
    cx.simulate_keystrokes("cmd-t");
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.workspace.tabs.len(), tabs);
            assert_eq!(shell.workspace.docks[&project].terminals.len(), 2);
            assert_eq!(shell.workspace.docks[&project].active, 1);
            assert_eq!(shell.dock_focused(w, cx), Some(1));
        })
        .unwrap();
    assert!(cx.debug_bounds("terminal-tab-1").is_some());
    // ⌘W closes the focused terminal; the session tab stays.
    cx.simulate_keystrokes("cmd-w");
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.tabs.len(), tabs);
            assert_eq!(shell.workspace.docks[&project].terminals.len(), 1);
            assert_eq!(shell.terminals[&project].len(), 1);
        })
        .unwrap();
    // Docked right, it takes width: a narrow window gives up its sidebar.
    cx.simulate_resize(gpui::size(px(1000.), px(800.)));
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, _, cx| {
            shell.workspace.docks.get_mut(&project).unwrap().size = 360.;
            shell.set_dock_side(DockSide::Right, cx);
            assert!(!shell.workspace.sidebar_visible);
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    let dock = cx.debug_bounds("terminal-dock").unwrap();
    assert!((dock.size.width - px(360.)).abs() < px(2.), "{dock:?}");
    // ⌘J hides it and the composer takes the keys back.
    window
        .update(cx, |shell, w, cx| shell.focus_terminal(w, cx))
        .unwrap();
    cx.simulate_keystrokes("cmd-j");
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    window
        .update(cx, |shell, w, cx| {
            assert!(!shell.workspace.docks[&project].open);
            assert_eq!(shell.dock_focused(w, cx), None);
            assert!(shell.workspace.sidebar_visible);
        })
        .unwrap();
    assert!(cx.debug_bounds("terminal-dock").is_none());
}
#[gpui::test]
fn closing_the_last_review_tab_removes_the_pane(cx: &mut TestAppContext) {
    let (window, _, b) = window(cx);
    let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(cx, |shell, window, cx| {
            shell.open(b, window, cx);
            shell.open_review(
                crate::git_tools::review::ReviewTab::Commit {
                    cwd: "/work/vorkspace".into(),
                    sha: "abcdef1".into(),
                    short_sha: "abcdef1".into(),
                    subject: "Fix".into(),
                },
                window,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.reviews.contains_key(&b));
            assert_eq!(shell.workspace.reviews[&b].tabs.len(), 1);
            // A chat with a file beside it: the chat's line, then the
            // file's, which heads the tab while it has focus.
            let label = shell.workspace.reviews[&b].tabs[0].label();
            let copy = shell.tab_copy(b, cx);
            assert_eq!(copy.headline, label);
            let keys: Vec<_> = copy.panes.iter().map(|p| p.key).collect();
            assert_eq!((keys, copy.focus), (vec![(b, false), (b, true)], 1));
            shell.review_focused = false;
            shell.sessions[&b].update(cx, |s, _| s.summary.title = "Fix login".into());
            let copy = shell.tab_copy(b, cx);
            assert_eq!(copy.headline, "Fix login");
            assert_eq!((copy.panes[0].title.as_str(), copy.focus), ("Fix login", 0));
            let review = shell.reviews[&b].clone();
            review.update(cx, |r, cx| r.close_active(cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.reviews.contains_key(&b));
            assert!(!shell.workspace.reviews.contains_key(&b));
            // Rendering again never rebuilds an empty pane.
            assert!(shell.review_for(b, cx).is_none());
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| assert!(!shell.reviews.contains_key(&b)))
        .unwrap();
}
#[gpui::test]
fn closing_and_reopening_tabs_preserves_busy_session_draft_and_history(cx: &mut TestAppContext) {
    let (window, a, b) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.sessions[&b].update(cx, |v, cx| {
                v.busy = true;
                v.input.update(cx, |i, cx| {
                    i.replace_text_in_range(None, "A retained draft 🙂", w, cx)
                });
            });
            shell.close(b, w, cx);
            assert_eq!(shell.workspace.active, None);
            assert_eq!(shell.workspace.project, "/work/vorkspace");
            assert!(shell.sessions[&b].read(cx).busy);
            shell.open(a, w, cx);
            shell.open(b, w, cx);
            assert_eq!(shell.workspace.active, Some(b));
            assert_eq!(
                shell.sessions[&b].read(cx).input.read(cx).content.as_ref(),
                "A retained draft 🙂"
            );
            shell.close(a, w, cx);
            shell.close(b, w, cx);
            assert!(shell.workspace.tabs.is_empty());
            assert!(shell.workspace.active.is_none());
            assert_eq!(shell.sessions.len(), 2);
            shell.open(b, w, cx);
            assert_eq!(shell.workspace.tabs, vec![b]);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let saved = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
            let saved: Workspace = serde_json::from_str(&saved).unwrap();
            assert_eq!(saved.tabs, vec![b]);
            assert_eq!(
                shell.sessions[&a]
                    .read(cx)
                    .transcript
                    .read(cx)
                    .list
                    .item_count(),
                4,
                "reopening a closed session must load its receipt, answer, actions and footer"
            );
        })
        .unwrap();
}
#[gpui::test]
fn typing_and_streaming_do_not_rebuild_sidebar_or_tabs(cx: &mut TestAppContext) {
    let (window, _, b) = window(cx);
    let root = window.root(cx).unwrap();
    let chrome = root.read_with(cx, |s, _| (s.sidebar_frame.clone(), s.tabs_frame.clone()));
    let before = (
        chrome.0.read_with(cx, |v, _| v.renders),
        chrome.1.read_with(cx, |v, _| v.renders),
    );
    assert!(
        before.0 > 0 && before.1 > 0,
        "the cached surfaces must have actually rendered"
    );
    window
        .update(cx, |shell, w, cx| {
            shell.sessions[&b].update(cx, |v, cx| {
                v.input.update(cx, |i, cx| {
                    i.replace_text_in_range(None, "Local keystrokes", w, cx)
                });
                v.transcript.update(cx, |t, cx| {
                    t.patch(
                        0,
                        vec![vorkspace_core::transcript::Row {
                            role: "assistant".into(),
                            text: "Streaming output".into(),
                            first: true,
                            sent_at: None,
                        }],
                        cx,
                    )
                });
                cx.notify();
            })
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    assert_eq!(
        chrome.0.read_with(cx, |v, _| v.renders),
        before.0,
        "chat changes must reuse the sidebar"
    );
    assert_eq!(
        chrome.1.read_with(cx, |v, _| v.renders),
        before.1,
        "chat changes must reuse the tabs"
    );
}

#[gpui::test]
fn settled_title_does_not_schedule_decorative_redraws(cx: &mut TestAppContext) {
    let (window, _, id) = window(cx);
    vorkspace_ui::clock::use_test_clock(cx);
    // Blank tabs deliberately have no sidebar card. Mount a conversation
    // before changing its title, so this exercises an existing label.
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&id].update(cx, |session, cx| {
                session.summary.has_messages = true;
                session.rename("Original session title".into(), cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    window
        .update(cx, |shell, _, cx| {
            shell.sessions[&id].update(cx, |session, cx| {
                session.rename("Updated session title".into(), cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    // Settle the title-change layout before measuring idle redraw work.
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.run_until_parked();
    let (sidebar, tabs, transcript) = window
        .read_with(cx, |shell, cx| {
            (
                shell.sidebar_frame.clone(),
                shell.tabs_frame.clone(),
                shell.sessions[&id].read(cx).transcript.clone(),
            )
        })
        .unwrap();
    let before = (
        sidebar.read_with(cx, |view, _| view.renders),
        tabs.read_with(cx, |view, _| view.renders),
        transcript.read_with(cx, |view, _| view.render_passes),
    );
    for _ in 0..3 {
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
    assert_eq!(sidebar.read_with(cx, |view, _| view.renders), before.0);
    assert_eq!(tabs.read_with(cx, |view, _| view.renders), before.1);
    assert_eq!(
        transcript.read_with(cx, |view, _| view.render_passes),
        before.2
    );
}

#[gpui::test]
fn reopening_activates_the_workspace_over_a_floating_window(cx: &mut TestAppContext) {
    let (workspace, _, _) = window(cx);
    let panel = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| cx.new(|_| gpui::Empty))
            .unwrap()
    });
    panel
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    let active = |cx: &mut TestAppContext| cx.update(|cx| workspace.is_active(cx));
    assert_eq!(active(cx), Some(false));
    cx.update(crate::app_menu::reopen_workspace);
    cx.run_until_parked();
    assert_eq!(active(cx), Some(true));
}
