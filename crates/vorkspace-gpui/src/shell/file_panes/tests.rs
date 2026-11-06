use super::*;
use crate::{git_tools::review::ReviewTab, shell::tests::window};
use gpui::{EntityInputHandler, TestAppContext, VisualTestContext, point};

#[gpui::test]
fn bot_file_link_opens_visible_editor_and_preserves_its_chat(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    check_bot_file_link(cx, 800., false);
}

#[gpui::test]
fn bot_file_link_tiles_beside_its_chat_when_space_allows(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    check_bot_file_link(cx, 1500., true);
}

fn check_bot_file_link(cx: &mut TestAppContext, width: f32, tiled: bool) {
    let (window, _, session) = window(cx);
    let folder = tempfile::tempdir().unwrap();
    let path = folder
        .path()
        .join("README.md")
        .to_string_lossy()
        .into_owned();
    std::fs::write(&path, "# Heading\nsecond line\nthird line\n").unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(width), px(800.)));
    window
        .update(visual, |shell, w, cx| {
            shell
                .bot_manager()
                .update(cx, |m, cx| m.create_named(Vec::new(), w, cx));
        })
        .unwrap();
    visual.run_until_parked();
    let (bot, chat) = window
        .update(visual, |shell, w, cx| {
            assert!(shell.bot_active(cx));
            let bot = shell.workspace.active.unwrap();
            let manager = shell.bot_manager();
            assert!(manager.update(cx, |m, cx| m.dismiss_settings(w, cx)));
            let chat = manager.read(cx).chat().unwrap();
            assert!(chat.read(cx).input.focus_handle(cx).is_focused(w));
            chat.read(cx).input.clone().update(cx, |input, cx| {
                input.set_text("Keep my Bot draft".into(), cx)
            });
            (bot, chat)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("bot-settings-dialog").is_none());
    assert!(
        visual.debug_bounds("composer-box").is_some(),
        "the action starts in the visible Bot"
    );
    window
        .update(visual, |_, w, cx| {
            w.dispatch_action(
                Box::new(crate::file_link::OpenPath(format!("{path}:2:3"))),
                cx,
            );
        })
        .unwrap();
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("review-tab-strip").is_some(),
        "the file pane is rendered"
    );
    if tiled {
        let composer = visual
            .debug_bounds("composer-box")
            .expect("Bot remains visible");
        let review = visual.debug_bounds("review-frame").unwrap();
        assert!(composer.right() <= review.left());
    } else {
        assert!(visual.debug_bounds("composer-box").is_none());
    }
    let (pane, editor) = window
        .update(visual, |shell, w, cx| {
            assert!(!shell.bot_active(cx));
            let pane = shell.workspace.active.unwrap();
            assert_eq!(shell.workspace.session_for_pane(pane), None);
            let expected_tab = if tiled { bot } else { pane };
            assert_eq!(shell.workspace.tab_of(pane), Some(expected_tab));
            if tiled {
                assert_eq!(shell.workspace.layout(bot).leaf_ids(), [bot, pane]);
            }
            assert!(
                shell.visible_tabs(cx).contains(&TitleTab::Workspace(bot)),
                "the Bot stays open"
            );
            assert!(
                shell
                    .visible_tabs(cx)
                    .contains(&TitleTab::Workspace(session))
            );
            let review = shell.active_review(cx).unwrap();
            assert!(review.read(cx).state.chat_hidden);
            let editor = review
                .read(cx)
                .active_editor()
                .expect("the file editor is mounted");
            assert_eq!(editor.read(cx).path, path);
            assert!(
                editor.read(cx).focus_handle(cx).contains_focused(w, cx),
                "focus is inside the visible editor"
            );
            assert_eq!(
                editor.read(cx).input.read(cx).selection_range(),
                12..12,
                "line and column survive navigation"
            );
            editor.read(cx).input.clone().update(cx, |input, cx| {
                input.replace_text_in_range(None, "unsaved", w, cx)
            });
            shell.open(bot, w, cx);
            assert!(
                chat.read(cx).input.focus_handle(cx).is_focused(w),
                "opening the Bot requests composer focus (tiled={tiled})"
            );
            (pane, editor)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("composer-box").is_some());
    window
        .update(visual, |shell, w, cx| {
            assert_eq!(shell.workspace.active, Some(bot));
            let restored = shell.bot_manager().read(cx).chat().unwrap();
            assert_eq!(
                restored, chat,
                "the original live Bot conversation is retained"
            );
            assert_eq!(
                restored.read(cx).input.read(cx).content.as_ref(),
                "Keep my Bot draft"
            );
            assert!(
                restored.read(cx).input.focus_handle(cx).is_focused(w),
                "the returning Bot composer receives focus (tiled={tiled}, focused={:?})",
                w.focused(cx)
            );
            shell.close_pane(pane, w, cx);
            assert_eq!(shell.workspace.tab_of(pane), None);
            assert!(restored.read(cx).input.focus_handle(cx).is_focused(w));
            w.dispatch_action(Box::new(crate::file_link::OpenPath(path.clone())), cx);
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, w, cx| {
            assert_eq!(shell.workspace.active, Some(pane));
            let reopened = shell
                .active_review(cx)
                .unwrap()
                .read(cx)
                .active_editor()
                .unwrap();
            assert_eq!(
                reopened, editor,
                "reopening keeps the existing dirty editor"
            );
            assert!(reopened.read(cx).dirty);
            assert!(reopened.read(cx).focus_handle(cx).contains_focused(w, cx));
            assert!(
                shell.visible_tabs(cx).contains(&TitleTab::Workspace(bot)),
                "reopening the file keeps the Bot"
            );
            reopened.update(cx, |_, cx| {
                cx.emit(crate::file_tools::file_editor::FileEvent::AddToChat(
                    "@README.md (line 2)".into(),
                ));
            });
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(
                chat.read(cx).input.read(cx).content,
                "Keep my Bot draft\n\n@README.md (line 2)\n\n"
            );
            assert!(
                shell.sessions[&session]
                    .read(cx)
                    .input
                    .read(cx)
                    .content
                    .is_empty()
            );
        })
        .unwrap();
}

#[gpui::test]
fn restored_bot_review_keeps_its_comment_destination_after_moving_beside_another_bot(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = window(cx);
    let repo = tempfile::tempdir().unwrap();
    let path = repo.path().join("review.txt");
    std::fs::write(&path, "before\n").unwrap();
    for args in [
        &["init", "-q"][..],
        &["add", "review.txt"][..],
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "base",
        ][..],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(&path, "after\n").unwrap();
    let cwd = repo.path().to_string_lossy().into_owned();
    let diff = ReviewTab::Changes {
        cwd: cwd.clone(),
        path: None,
        staged: Some(false),
    };
    window
        .update(cx, |shell, w, cx| {
            shell
                .bot_manager()
                .update(cx, |manager, cx| manager.create_named(Vec::new(), w, cx));
        })
        .unwrap();
    cx.run_until_parked();
    let (bot, original_pane, review_pane, review) = window
        .update(cx, |shell, w, cx| {
            let bot_pane = shell.workspace.active.unwrap();
            let manager = shell.bot_manager();
            let bot = manager.read(cx).current().unwrap().id.clone();
            shell.open_review(diff.clone(), w, cx);
            let pane = shell.workspace.active.unwrap();
            let review = shell.active_review(cx).unwrap();
            shell.open_review(
                ReviewTab::File {
                    cwd,
                    path: "review.txt".into(),
                },
                w,
                cx,
            );
            assert_eq!(shell.workspace.active, Some(pane));
            assert_eq!(
                shell.workspace.app_panes[&pane].review_bot.as_deref(),
                Some(bot.as_str())
            );
            shell.open_review(diff.clone(), w, cx);
            assert_eq!(shell.workspace.active, Some(pane));
            manager.update(cx, |manager, cx| manager.create_named(Vec::new(), w, cx));
            (bot, bot_pane, pane, review)
        })
        .unwrap();
    cx.run_until_parked();
    let other = window
        .update(cx, |shell, w, cx| {
            let other = shell.workspace.active.unwrap();
            assert_ne!(other, original_pane);
            shell.open_review(diff, w, cx);
            let other_review = shell.workspace.active.unwrap();
            assert_ne!(other_review, review_pane);
            shell.close_pane(other_review, w, cx);
            shell.workspace.place(review_pane, other, Edge::Right);
            shell.open(review_pane, w, cx);
            shell.close_pane(review_pane, w, cx);
            shell.open(review_pane, w, cx);
            assert_eq!(shell.active_review(cx), Some(review));
            shell.workspace.place(review_pane, other, Edge::Right);
            shell.open(review_pane, w, cx);
            other
        })
        .unwrap();
    cx.run_until_parked();
    let (store, summaries, project, saved) = window
        .update(cx, |shell, w, _| {
            let (summaries, saved) = {
                let db = locked(&shell.store).unwrap();
                (db.sessions().unwrap(), db.ui_state().unwrap())
            };
            w.remove_window();
            (
                shell.store.clone(),
                summaries,
                shell.workspace.project.clone(),
                saved,
            )
        })
        .unwrap();
    let restored = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(store, summaries, project, saved, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    let (view, draft, manager, other_chat) = restored
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.workspace.active, Some(review_pane));
            assert_eq!(
                shell.workspace.layout(other).leaf_ids(),
                [other, review_pane]
            );
            assert_eq!(
                shell.workspace.app_panes[&review_pane]
                    .review_bot
                    .as_deref(),
                Some(bot.as_str())
            );
            let manager = shell.bot_manager();
            assert_ne!(manager.read(cx).current().unwrap().id, bot);
            let other_chat = manager.read(cx).chat().unwrap();
            let view = shell
                .active_review(cx)
                .unwrap()
                .read(cx)
                .active_diff_for_test();
            let draft = view.update(cx, |view, cx| view.open_comment_for_test(w, cx));
            draft.update(cx, |input, cx| {
                input.set_text("A review comment".into(), cx);
                cx.emit(crate::input::Submitted);
            });
            (view, draft, manager, other_chat)
        })
        .unwrap();
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(
            view.comment_error_for_test()
                .unwrap()
                .contains("wait for its draft to load")
        );
    });
    let chat = manager.update(cx, |manager, cx| manager.chat_for_action(&bot, cx).unwrap());
    restored
        .update(cx, |_, w, cx| {
            chat.read(cx).input.clone().update(cx, |input, cx| {
                input.set_text("Keep this draft".into(), cx);
                input.replace_and_mark_text_in_range(None, " while typing", None, w, cx);
            });
            draft.update(cx, |_, cx| cx.emit(crate::input::Submitted));
        })
        .unwrap();
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(
            view.comment_error_for_test()
                .unwrap()
                .contains("Finish typing")
        );
    });
    draft.read_with(cx, |draft, _| assert_eq!(draft.content, "A review comment"));
    restored
        .update(cx, |_, w, cx| {
            chat.read(cx)
                .input
                .clone()
                .update(cx, |input, cx| input.unmark_text(w, cx));
            draft.update(cx, |_, cx| {
                cx.emit(crate::input::Submitted);
                cx.emit(crate::input::Submitted);
            });
        })
        .unwrap();
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(!view.has_comment_draft()));
    chat.read_with(cx, |chat, cx| {
        let text = &chat.input.read(cx).content;
        assert!(text.starts_with("Keep this draft while typing\n\n"));
        assert_eq!(text.matches("A review comment").count(), 1);
        assert!(!chat.busy);
    });
    other_chat.read_with(cx, |chat, cx| {
        assert!(chat.input.read(cx).content.is_empty())
    });

    let missing_draft = restored
        .update(cx, |_, w, cx| {
            let draft = view.update(cx, |view, cx| view.open_comment_for_test(w, cx));
            manager.update(cx, |manager, _| {
                manager.roster.bots.retain(|actor| actor.id != bot)
            });
            draft.update(cx, |input, cx| {
                input.set_text("Keep me if the Bot is gone".into(), cx);
                cx.emit(crate::input::Submitted);
            });
            draft
        })
        .unwrap();
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(
            view.comment_error_for_test()
                .unwrap()
                .contains("Bot is unavailable")
        );
    });
    missing_draft.read_with(cx, |draft, _| {
        assert_eq!(draft.content, "Keep me if the Bot is gone")
    });
    chat.read_with(cx, |chat, cx| {
        assert!(!chat.input.read(cx).content.contains("Keep me"))
    });
    other_chat.read_with(cx, |chat, cx| {
        assert!(chat.input.read(cx).content.is_empty())
    });
}

#[gpui::test]
fn file_word_wrap_is_shared_and_persisted_across_restart(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let folder = std::env::temp_dir().join(format!("vorkspace-wrap-prefs-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    for file in ["one.txt", "two.txt"] {
        std::fs::write(folder.join(file), "a long line ".repeat(50)).unwrap();
    }
    window
        .update(cx, |shell, w, cx| {
            shell.open_review(
                ReviewTab::File {
                    cwd: shell.workspace.project.clone(),
                    path: folder.join("one.txt").to_string_lossy().into_owned(),
                },
                w,
                cx,
            );
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |_, w, cx| {
            w.dispatch_action(Box::new(crate::file_tools::file_editor::ToggleWordWrap), cx)
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, w, cx| {
            assert!(!shell.workspace.preferences.file_word_wrap);
            assert!(
                !shell.reviews[&session]
                    .read(cx)
                    .active_editor()
                    .unwrap()
                    .read(cx)
                    .input
                    .read(cx)
                    .word_wrap()
            );
            shell.open_review(
                ReviewTab::File {
                    cwd: shell.workspace.project.clone(),
                    path: folder.join("two.txt").to_string_lossy().into_owned(),
                },
                w,
                cx,
            );
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let (store, summaries, saved, project) = window
        .update(visual, |shell, _, cx| {
            assert!(
                !shell.reviews[&session]
                    .read(cx)
                    .active_editor()
                    .unwrap()
                    .read(cx)
                    .input
                    .read(cx)
                    .word_wrap(),
                "new files inherit the setting"
            );
            let store = locked(&shell.store).unwrap();
            let saved = store.ui_state().unwrap();
            let persisted: Workspace = serde_json::from_str(saved.as_ref().unwrap()).unwrap();
            assert!(!persisted.preferences.file_word_wrap);
            (
                shell.store.clone(),
                store.sessions().unwrap(),
                saved,
                shell.workspace.project.clone(),
            )
        })
        .unwrap();
    let restored = visual.update(|_, cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(store, summaries, project, saved, cx))
        })
        .unwrap()
    });
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    visual.refresh().unwrap();
    restored
        .update(visual, |shell, _, cx| {
            assert!(!shell.workspace.preferences.file_word_wrap);
            let editor = shell.reviews[&session].read(cx).active_editor().unwrap();
            assert!(
                !editor.read(cx).input.read(cx).word_wrap(),
                "restart restores the setting"
            );
        })
        .unwrap();
    std::fs::remove_dir_all(folder).unwrap();
}

#[gpui::test]
fn numbered_shortcuts_return_from_file_only_tabs_to_sessions(cx: &mut TestAppContext) {
    cx.update(|cx| {
        bind_keys(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    let modifier = "cmd";
    // File-only tabs can retain a hidden chat or be detached from it.
    for detached in [false, true] {
        let (window, first, session) = window(cx);
        let file = window
            .update(cx, |shell, w, cx| {
                shell.sessions[&first]
                    .update(cx, |s, _| s.summary.cwd = shell.workspace.project.clone());
                shell.open(first, w, cx);
                shell.open_review(
                    ReviewTab::File {
                        cwd: shell.workspace.project.clone(),
                        path: "/missing/shortcut-test.txt".into(),
                    },
                    w,
                    cx,
                );
                shell.open_review(
                    ReviewTab::Changes {
                        cwd: shell.workspace.project.clone(),
                        path: None,
                        staged: None,
                    },
                    w,
                    cx,
                );
                let file = if detached {
                    let file = shell.independent_file_pane(first, cx).unwrap();
                    shell.workspace.close(first);
                    shell.reorder_title_tab(&TitleTab::Workspace(file), 0, cx);
                    file
                } else {
                    shell.reviews[&first].update(cx, |r, cx| {
                        r.state.chat_hidden = true;
                        cx.emit(crate::git_tools::review::ReviewEvent::Changed);
                    });
                    first
                };
                shell.open(session, w, cx);
                assert_eq!(
                    shell.visible_tabs(cx),
                    [TitleTab::Workspace(file), TitleTab::Workspace(session)]
                );
                file
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        for (nested_tab, restored) in [(1, false), (0, false), (1, true)] {
            if restored {
                window
                    .update(visual, |shell, _, _| {
                        // A restored tab has saved state before its pane is mounted.
                        shell.reviews.remove(&file);
                        shell.workspace.reviews.get_mut(&file).unwrap().active = nested_tab;
                    })
                    .unwrap();
            }
            visual.simulate_keystrokes(&format!("{modifier}-1"));
            visual.run_until_parked();
            window
                .update(visual, |shell, _, _| {
                    assert_eq!(shell.workspace.active, Some(file));
                })
                .unwrap();
            if nested_tab == 0 {
                let tab = visual.debug_bounds("file-tab-0").unwrap();
                visual.simulate_click(tab.center(), Default::default());
                visual.run_until_parked();
            }
            visual.simulate_keystrokes(&format!("{modifier}-2"));
            visual.run_until_parked();
            window
                    .update(visual, |shell, w, cx| {
                        assert_eq!(
                            shell.workspace.active,
                            Some(session),
                            "Cmd+2 returns from nested tab {nested_tab} (detached={detached}, restored={restored})"
                        );
                        assert!(
                            shell.sessions[&session]
                                .read(cx)
                                .input
                                .focus_handle(cx)
                                .is_focused(w)
                        );
                    })
                    .unwrap();
        }
    }
}

#[gpui::test]
fn session_pane_grip_detaches_and_redocks_preserving_chat_and_dirty_files(cx: &mut TestAppContext) {
    cx.update(|cx| {
        bind_keys(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    let (window, _, session) = window(cx);
    let path = std::env::temp_dir().join(format!(
        "vorkspace-chat-pane-drag-{}.txt",
        std::process::id()
    ));
    std::fs::write(&path, "original\n").unwrap();
    let (chat, review) = window
        .update(cx, |shell, w, cx| {
            let chat = shell.sessions[&session].clone();
            chat.read(cx).input.clone().update(cx, |input, cx| {
                input.set_text("Keep this draft".into(), cx);
            });
            shell.open_review(
                ReviewTab::File {
                    cwd: shell.workspace.project.clone(),
                    path: path.to_string_lossy().into_owned(),
                },
                w,
                cx,
            );
            (chat, shell.reviews[&session].clone())
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    let editor = window
        .update(cx, |_, w, cx| {
            let editor = review.read(cx).active_editor().unwrap();
            editor.read(cx).input.clone().update(cx, |input, cx| {
                input.replace_text_in_range(None, "unsaved ", w, cx);
            });
            editor
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let grip_selector = Box::leak(format!("drag-session-pane-{session}").into_boxed_str());
    let grip = visual.debug_bounds(grip_selector).unwrap().center();

    // Clicks, tiny moves, self drops, outside drops and Escape do not
    // split the attached file from its session.
    for end in [
        grip,
        grip + point(px(2.), px(0.)),
        grip + point(px(30.), px(0.)),
        point(px(6.), px(100.)),
    ] {
        visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    }
    let strip = visual
        .debug_bounds(Box::leak(format!("title-tab-{session}").into_boxed_str()))
        .unwrap();
    let before = point(strip.left() + px(8.), strip.center().y);
    visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(before, MouseButton::Left, Default::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(before, MouseButton::Left, Default::default());
    window
        .update(visual, |shell, _, _| {
            assert!(shell.workspace.review_owners.is_empty());
            assert_eq!(shell.reviews[&session], review);
            assert!(shell.pane_drag.is_none());
        })
        .unwrap();

    // Pulling the chat onto the title strip leaves the file in its tab.
    visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(before, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(before, MouseButton::Left, Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let file = window
        .update(visual, |shell, w, cx| {
            let file = *shell.workspace.review_owners.keys().next().unwrap();
            assert_eq!(
                shell.visible_tabs(cx),
                [TitleTab::Workspace(session), TitleTab::Workspace(file)]
            );
            assert_eq!(shell.workspace.active, Some(session));
            assert!(!shell.reviews.contains_key(&session));
            assert_eq!(shell.reviews[&file], review);
            shell.open(file, w, cx);
            file
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);

    // Redock the session next to the file, then reorder it with its grip.
    let tab = visual
        .debug_bounds(Box::leak(format!("title-tab-{session}").into_boxed_str()))
        .unwrap()
        .center();
    let bounds = window
        .update(visual, |shell, _, _| shell.pane_bounds.get().unwrap())
        .unwrap();
    let right = point(bounds.right() - px(12.), bounds.center().y);
    visual.simulate_mouse_down(tab, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(right, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(right, MouseButton::Left, Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.layout(file).leaf_ids(), [file, session]);
        })
        .unwrap();
    let grip = visual.debug_bounds(grip_selector).unwrap().center();
    let left = point(bounds.left() + px(12.), bounds.center().y);
    visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(left, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(left, MouseButton::Left, Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(shell.workspace.layout(file).leaf_ids(), [session, file]);
            assert_eq!(shell.workspace.active, Some(session));
            assert_eq!(shell.sessions[&session], chat);
            assert_eq!(
                chat.read(cx).input.read(cx).content.as_ref(),
                "Keep this draft"
            );
            assert_eq!(shell.reviews[&file], review);
            assert_eq!(review.read(cx).active_editor().unwrap(), editor);
            assert!(editor.read(cx).dirty);
            let saved = serde_json::to_string(&shell.workspace).unwrap();
            let sessions: Vec<_> = shell
                .sessions
                .iter()
                .map(|(id, s)| (*id, s.read(cx).summary.cwd.clone()))
                .collect();
            let restored = Workspace::restore(Some(&saved), &sessions, &shell.workspace.project);
            assert_eq!(restored.layout(file), shell.workspace.layout(file));
            assert_eq!(restored.active, Some(session));
            assert_eq!(restored.review_owners, shell.workspace.review_owners);
        })
        .unwrap();
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn session_pane_grip_places_chat_on_each_edge_of_its_attached_file(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        let (window, _, session) = window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open_review(
                    ReviewTab::File {
                        cwd: shell.workspace.project.clone(),
                        path: "/tmp/chat-pane-edge.rs".into(),
                    },
                    w,
                    cx,
                );
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        let grip = visual
            .debug_bounds(Box::leak(
                format!("drag-session-pane-{session}").into_boxed_str(),
            ))
            .unwrap()
            .center();
        let file_bounds = visual.debug_bounds("review-frame").unwrap();
        let end = match edge {
            Edge::Left => point(file_bounds.left() + px(12.), file_bounds.center().y),
            Edge::Right => point(file_bounds.right() - px(12.), file_bounds.center().y),
            Edge::Top => point(file_bounds.center().x, file_bounds.top() + px(12.)),
            Edge::Bottom => point(file_bounds.center().x, file_bounds.bottom() - px(12.)),
        };
        visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
        window
            .update(visual, |shell, _, _| {
                assert_eq!(
                    shell.pane_drag.as_ref().unwrap().over,
                    Some((session, edge))
                );
            })
            .unwrap();
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        window
            .update(visual, |shell, _, _| {
                let file = *shell.workspace.review_owners.keys().next().unwrap();
                let tab = shell.workspace.tab_of(session).unwrap();
                let leaves = shell.workspace.layout(tab).leaves();
                let chat_rect = leaves.iter().find(|l| l.id == session).unwrap().rect;
                let file_rect = leaves.iter().find(|l| l.id == file).unwrap().rect;
                assert_eq!(shell.workspace.active, Some(session));
                assert!(match edge {
                    Edge::Left => chat_rect.x < file_rect.x,
                    Edge::Right => chat_rect.x > file_rect.x,
                    Edge::Top => chat_rect.y < file_rect.y,
                    Edge::Bottom => chat_rect.y > file_rect.y,
                });
            })
            .unwrap();
    }
}

#[gpui::test]
fn file_pane_grip_detaches_and_redocks_without_recreating_dirty_editors(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let path = std::env::temp_dir().join(format!("vorkspace-pane-drag-{}.txt", std::process::id()));
    std::fs::write(&path, "original\n").unwrap();
    let file = path.to_string_lossy().into_owned();
    let review = window
        .update(cx, |shell, w, cx| {
            shell.open_review(
                ReviewTab::File {
                    cwd: shell.workspace.project.clone(),
                    path: file.clone(),
                },
                w,
                cx,
            );
            shell.reviews[&session].clone()
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, cx);
    let editor = window
        .update(cx, |_, w, cx| {
            let editor = review.read(cx).active_editor().unwrap();
            let input = editor.read(cx).input.clone();
            input.update(cx, |i, cx| i.replace_text_in_range(None, "unsaved ", w, cx));
            editor
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let grip = visual.debug_bounds("drag-file-pane").unwrap().center();
    // A click and a drop outside the workspace leave the pane attached.
    visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(grip, MouseButton::Left, Default::default());
    visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
    let outside = point(px(6.), px(100.));
    visual.simulate_mouse_move(outside, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(outside, MouseButton::Left, Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, _, _| {
            assert!(shell.workspace.review_owners.is_empty());
            assert_eq!(shell.reviews[&session], review);
        })
        .unwrap();
    let tab = visual
        .debug_bounds(Box::leak(format!("title-tab-{session}").into_boxed_str()))
        .unwrap();
    let drop = point(tab.left() + px(8.), tab.center().y);
    visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(drop, MouseButton::Left, Default::default());
    window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.pane_drag.as_ref().unwrap().insert, Some(0));
        })
        .unwrap();
    visual.simulate_mouse_up(drop, MouseButton::Left, Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let pane = window
        .update(visual, |shell, _, cx| {
            let pane = shell.workspace.active.unwrap();
            assert!(pane < 0);
            assert_eq!(shell.workspace.session_for_pane(pane), Some(session));
            assert!(!shell.reviews.contains_key(&session));
            assert_eq!(shell.reviews[&pane], review);
            assert_eq!(review.read(cx).active_editor().unwrap(), editor);
            assert!(editor.read(cx).dirty);
            assert_eq!(
                shell.visible_tabs(cx),
                [TitleTab::Workspace(pane), TitleTab::Workspace(session)]
            );
            assert_eq!(
                shell.tab_copy(pane, cx).headline,
                path.file_name().unwrap().to_string_lossy()
            );
            let saved = serde_json::to_string(&shell.workspace).unwrap();
            let sessions: Vec<_> = shell
                .sessions
                .iter()
                .map(|(id, s)| (*id, s.read(cx).summary.cwd.clone()))
                .collect();
            let restored = Workspace::restore(Some(&saved), &sessions, &shell.workspace.project);
            assert_eq!(restored.active, Some(pane));
            assert_eq!(restored.tabs, shell.workspace.tabs);
            assert_eq!(restored.review_owners, shell.workspace.review_owners);
            pane
        })
        .unwrap();
    assert!(
        visual
            .debug_bounds(Box::leak(format!("title-tab-{pane}").into_boxed_str()))
            .is_some()
    );

    // A closed title tab retains its dirty documents for reopening.
    window
        .update(visual, |shell, w, cx| shell.close(pane, w, cx))
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, w, cx| {
            assert!(shell.workspace.tab_of(pane).is_none());
            shell.open_review(
                ReviewTab::File {
                    cwd: shell.workspace.project.clone(),
                    path: file.clone(),
                },
                w,
                cx,
            );
            assert_eq!(shell.workspace.active, Some(pane));
            assert_eq!(shell.reviews[&pane], review);
            assert_eq!(review.read(cx).active_editor().unwrap(), editor);
            assert!(editor.read(cx).dirty);
        })
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);

    // The independent file tab can be pulled back into a split.
    window
        .update(visual, |shell, w, cx| shell.open(session, w, cx))
        .unwrap();
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let tab = visual
        .debug_bounds(Box::leak(format!("title-tab-{pane}").into_boxed_str()))
        .unwrap()
        .center();
    let bounds = window
        .update(visual, |shell, _, _| shell.pane_bounds.get().unwrap())
        .unwrap();
    let left = point(bounds.left() + px(12.), bounds.center().y);
    visual.simulate_mouse_down(tab, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(left, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(left, MouseButton::Left, Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(shell.workspace.layout(session).leaf_ids(), [pane, session]);
            assert_eq!(shell.reviews[&pane], review);
            assert!(editor.read(cx).dirty);
        })
        .unwrap();

    // Its grip also reorders panes within the split.
    let grip = visual.debug_bounds("drag-file-pane").unwrap().center();
    let right = point(bounds.right() - px(12.), bounds.center().y);
    visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(right, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(right, MouseButton::Left, Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, _, cx| {
            assert_eq!(shell.workspace.layout(session).leaf_ids(), [session, pane]);
            assert_eq!(review.read(cx).active_editor().unwrap(), editor);
            assert!(editor.read(cx).dirty);
            let saved = serde_json::to_string(&shell.workspace).unwrap();
            let sessions: Vec<_> = shell
                .sessions
                .iter()
                .map(|(id, s)| (*id, s.read(cx).summary.cwd.clone()))
                .collect();
            let restored = Workspace::restore(Some(&saved), &sessions, &shell.workspace.project);
            assert_eq!(restored.layout(session).leaf_ids(), [session, pane]);
            assert_eq!(restored.active, Some(pane));
            let remaining: Vec<_> = sessions
                .into_iter()
                .filter(|(id, _)| *id != session)
                .collect();
            let pruned = Workspace::restore(Some(&saved), &remaining, &shell.workspace.project);
            assert!(!pruned.review_owners.contains_key(&pane));
            assert!(!pruned.reviews.contains_key(&pane));
            assert!(pruned.tab_of(pane).is_none());
        })
        .unwrap();
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn file_tab_drag_reorders_and_keeps_the_selected_editor(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let review = window
        .update(cx, |shell, w, cx| {
            for name in ["a.rs", "b.rs", "c.rs"] {
                shell.open_review(
                    ReviewTab::File {
                        cwd: shell.workspace.project.clone(),
                        path: format!("/tmp/{name}"),
                    },
                    w,
                    cx,
                );
            }
            shell.reviews[&session].clone()
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    let first = visual.debug_bounds("file-tab-0").unwrap();
    let last = visual.debug_bounds("file-tab-2").unwrap();
    let start = point(first.left() + px(20.), first.center().y);
    let end = start + point(last.left() - first.left(), px(0.));
    visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
    let editor = window
        .update(visual, |_, _, cx| review.read(cx).active_editor().unwrap())
        .unwrap();
    visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    window
        .update(visual, |shell, _, cx| {
            let state = &review.read(cx).state;
            assert_eq!(
                state.tabs.iter().map(ReviewTab::label).collect::<Vec<_>>(),
                ["b.rs", "c.rs", "a.rs"]
            );
            assert_eq!(state.active, 2);
            assert_eq!(shell.workspace.reviews[&session].active, 2);
            assert_eq!(review.read(cx).active_editor().unwrap(), editor);
        })
        .unwrap();
}

#[gpui::test]
fn file_pane_grip_places_an_embedded_review_on_each_edge(cx: &mut TestAppContext) {
    vorkspace_ui::clock::use_test_clock(cx);
    let (window, _, session) = window(cx);
    let review = window
        .update(cx, |shell, w, cx| {
            shell.open_review(
                ReviewTab::File {
                    cwd: shell.workspace.project.clone(),
                    path: "/tmp/pane-edge.rs".into(),
                },
                w,
                cx,
            );
            shell.reviews[&session].clone()
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    vorkspace_ui::clock::settle_animations(window, visual);
    for edge in [Edge::Left, Edge::Top, Edge::Bottom, Edge::Right] {
        let grip = visual.debug_bounds("drag-file-pane").unwrap().center();
        let target = visual
            .debug_bounds(Box::leak(format!("pane-{session}").into_boxed_str()))
            .unwrap_or_else(|| {
                window
                    .update(visual, |shell, _, _| shell.pane_bounds.get().unwrap())
                    .unwrap()
            });
        let end = match edge {
            Edge::Left => point(target.left() + px(12.), target.center().y),
            Edge::Right => point(target.right() - px(12.), target.center().y),
            Edge::Top => point(target.center().x, target.top() + px(12.)),
            Edge::Bottom => point(target.center().x, target.bottom() - px(12.)),
        };
        visual.simulate_mouse_down(grip, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
        visual.run_until_parked();
        vorkspace_ui::clock::settle_animations(window, visual);
        window
            .update(visual, |shell, _, _| {
                let pane = shell.workspace.active.unwrap();
                assert_eq!(shell.workspace.review_owners.len(), 1);
                assert_eq!(shell.reviews[&pane], review);
                assert!(!shell.workspace.reviews.contains_key(&session));
                let leaves = shell.workspace.layout(session).leaves();
                let chat = leaves.iter().find(|l| l.id == session).unwrap().rect;
                let file = leaves.iter().find(|l| l.id == pane).unwrap().rect;
                assert!(match edge {
                    Edge::Left => file.x < chat.x,
                    Edge::Right => file.x > chat.x,
                    Edge::Top => file.y < chat.y,
                    Edge::Bottom => file.y > chat.y,
                });
            })
            .unwrap();
    }
}
