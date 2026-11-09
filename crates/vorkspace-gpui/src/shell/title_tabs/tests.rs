use super::*;
use crate::{shell::tests::window, workspace::AppView};
use gpui::TestAppContext;

#[test]
#[ignore = "Requires Metal and VOKKY_CONNECTOR_CAPTURE_DIR"]
fn connector_native_navigation_capture() {
    let output = std::path::PathBuf::from(std::env::var_os("VOKKY_CONNECTOR_CAPTURE_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let store = Arc::new(Mutex::new(
        vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    let session = locked(&store)
        .unwrap()
        .create("codex", "/work/connector-fixture")
        .unwrap();
    let mut state = Workspace::default();
    state.open(session.id);
    let saved = serde_json::to_string(&state).unwrap();
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
    let window = cx
        .open_window(gpui::size(px(1280.), px(800.)), |_, cx| {
            cx.new(|cx| {
                Shell::new(
                    store,
                    vec![session],
                    "/work/connector-fixture".into(),
                    Some(saved),
                    cx,
                )
            })
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(&mut cx, |shell, w, cx| {
            shell.ensure_background_inbox(w, cx);
            shell.open_connector(Connector::Github, w, cx);
            shell.open_connector(Connector::Jira, w, cx);
        })
        .unwrap();
    for (name, expanded) in [("expanded", true), ("compact", false)] {
        window
            .update(&mut cx, |shell, _, cx| {
                shell.workspace.project_rail_open = expanded;
                shell.invalidate_chrome(cx);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(output.join(format!("connectors-{name}.png")))
            .unwrap();
    }
    window
        .update(&mut cx, |shell, w, cx| {
            shell.open_settings(crate::settings::Section::Connectors, w, cx)
        })
        .unwrap();
    cx.run_until_parked();
    vorkspace_ui::clock::settle_headless_animations(window, &mut cx);
    cx.capture_screenshot(window.into())
        .unwrap()
        .save(output.join("connectors-settings.png"))
        .unwrap();
}
#[gpui::test]
fn command_w_closes_workspace_pages_and_restores_conversation(cx: &mut TestAppContext) {
    cx.update(|cx| {
        bind_keys(cx);
        crate::input::bind(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    let (window, _, session) = window(cx);
    let chat = window
        .update(cx, |shell, w, cx| {
            shell.open(session, w, cx);
            let chat = shell.sessions[&session].clone();
            chat.update(cx, |chat, cx| {
                chat.busy = true;
                chat.input.update(cx, |input, cx| {
                    input.set_text("unsent conversation draft".into(), cx)
                });
            });
            shell.focus_active(w, cx);
            chat
        })
        .unwrap();
    for page in [
        TitleTab::Search,
        TitleTab::App(AppView::Connector(Connector::Github)),
        TitleTab::App(AppView::Connector(Connector::Gitlab)),
        TitleTab::App(AppView::Connector(Connector::Jira)),
        TitleTab::App(AppView::Notes),
        TitleTab::App(AppView::Automations),
    ] {
        let strip = window
            .update(cx, |shell, w, cx| {
                let strip = shell.workspace.title_order.clone();
                shell.open_title_tab(&page, w, cx);
                if let Some(search) = &shell.app_search {
                    search
                        .view
                        .read(cx)
                        .input
                        .clone()
                        .update(cx, |input, cx| input.set_text("saved search".into(), cx));
                }
                strip
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let opened = shell.active_title_tab(cx).unwrap();
                assert!(matches!(opened, TitleTab::Workspace(id) if id != session));
                assert!(shell.workspace.title_order.contains(&opened));
                assert!(shell.visible_tabs(cx).contains(&opened));
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "cmd-w");
        cx.run_until_parked();
        window
            .update(cx, |shell, w, cx| {
                assert_eq!(
                    shell.active_title_tab(cx),
                    Some(TitleTab::Workspace(session))
                );
                assert_eq!(shell.workspace.title_order, strip);
                assert_eq!(shell.sessions[&session], chat);
                let chat = chat.read(cx);
                assert!(chat.busy, "page close must not cancel parked work");
                assert_eq!(chat.input.read(cx).content, "unsent conversation draft");
                assert!(chat.input.focus_handle(cx).is_focused(w), "{page:?}");
                if page == TitleTab::Search {
                    assert_eq!(shell.saved_search.as_ref().unwrap().0, "saved search");
                }
            })
            .unwrap();
    }
}

#[gpui::test]
fn search_opened_from_a_connector_returns_to_that_connector(cx: &mut TestAppContext) {
    cx.update(|cx| {
        bind_keys(cx);
        crate::input::bind(cx);
    });
    let (window, _, _) = window(cx);
    let connector = window
        .update(cx, |shell, w, cx| {
            shell.open_connector(Connector::Github, w, cx);
            shell.active_title_tab(cx).unwrap()
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| shell.open_app_search(w, cx))
        .unwrap();
    cx.run_until_parked();
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(shell.active_title_tab(cx), Some(connector.clone()));
            assert_eq!(shell.workspace.active, connector.session());
            assert!(
                shell.connector_pages[&Connector::Github]
                    .focus_handle(cx)
                    .is_focused(w)
            );
        })
        .unwrap();
}

#[gpui::test]
fn command_w_parks_bot_work_and_draft_and_restores_conversation(cx: &mut TestAppContext) {
    cx.update(|cx| {
        bind_keys(cx);
        crate::input::bind(cx);
        vorkspace_ui::clock::use_test_clock(cx);
    });
    let (window, _, session) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.bot_manager().update(cx, |bots, cx| {
                bots.create_named(vec!["/work/vorkspace".into()], w, cx)
            });
        })
        .unwrap();
    cx.run_until_parked();
    let (bot_tab, chat) = window
        .update(cx, |shell, _, cx| {
            let manager = shell.bot_manager();
            let bots = manager.read(cx);
            let bot_tab = shell.active_title_tab(cx).unwrap();
            let chat = bots.chat().unwrap();
            assert_eq!(
                shell.visible_tabs(cx),
                [TitleTab::Workspace(session), bot_tab.clone()]
            );
            chat.update(cx, |chat, cx| {
                chat.busy = true;
                chat.input.update(cx, |input, cx| {
                    input.set_text("unsent bot draft".into(), cx)
                });
            });
            (bot_tab, chat)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "cmd-w");
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert!(!shell.bot_active(cx));
            assert!(!shell.workspace.title_order.contains(&bot_tab));
            assert_eq!(
                shell.active_title_tab(cx),
                Some(TitleTab::Workspace(session))
            );
            assert!(
                shell.sessions[&session]
                    .read(cx)
                    .input
                    .focus_handle(cx)
                    .is_focused(w)
            );
            assert!(chat.read(cx).busy);
            assert_eq!(chat.read(cx).input.read(cx).content, "unsent bot draft");
            shell.open_title_tab(&bot_tab, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.bot_manager().read(cx).chat(), Some(chat.clone()));
            assert_eq!(shell.active_title_tab(cx), Some(bot_tab));
            assert_eq!(
                shell.visible_tabs(cx).len(),
                2,
                "reopening creates one Bot tab"
            );
            assert!(chat.read(cx).busy);
            assert_eq!(chat.read(cx).input.read(cx).content, "unsent bot draft");
        })
        .unwrap();
}

#[gpui::test]
fn bot_tabs_restore_order_selection_and_project_scope(cx: &mut TestAppContext) {
    let (window, _, session) = window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.bot_manager().update(cx, |m, cx| {
                m.create_named(vec!["/work/vorkspace".into()], w, cx);
                m.dismiss_settings(w, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let bot = window
        .update(cx, |shell, _, cx| {
            let bot = shell.active_title_tab(cx).unwrap();
            shell.reorder_title_tab(&bot, 0, cx);
            shell.persist(cx);
            bot
        })
        .unwrap();
    cx.run_until_parked();
    let (store, summaries, saved) = window
        .update(cx, |shell, _, _| {
            let db = locked(&shell.store).unwrap();
            (
                shell.store.clone(),
                db.sessions().unwrap(),
                db.ui_state().unwrap(),
            )
        })
        .unwrap();
    let reopened = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(store, summaries, "/work/vorkspace".into(), saved, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    reopened
        .update(cx, |shell, w, cx| {
            assert_eq!(
                shell.visible_tabs(cx),
                [bot.clone(), TitleTab::Workspace(session)]
            );
            assert_eq!(shell.active_title_tab(cx), Some(bot.clone()));
            let chat = shell.bot_manager().read(cx).chat().unwrap();
            assert!(chat.read(cx).input.focus_handle(cx).is_focused(w));
            shell.open(session, w, cx);
            shell.select_project("/work/another-project".into(), w, cx);
            assert!(!shell.visible_tabs(cx).contains(&bot));
            assert!(shell.workspace.title_order.contains(&bot));
            shell.select_project("/work/vorkspace".into(), w, cx);
            assert_eq!(
                shell.visible_tabs(cx),
                [bot.clone(), TitleTab::Workspace(session)]
            );
            shell.open_title_tab(&bot, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    reopened
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.active_title_tab(cx), Some(bot.clone()));
            assert_eq!(shell.visible_tabs(cx), [bot, TitleTab::Workspace(session)]);
        })
        .unwrap();
}

#[gpui::test]
fn rapid_new_tabs_follow_request_order(cx: &mut TestAppContext) {
    let (window, _, first) = window(cx);
    let providers = [Kind::Claude, Kind::Codex, Kind::Claude, Kind::Codex];
    window
        .update(cx, |shell, _, cx| {
            for provider in providers {
                shell.create(provider, cx);
            }
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let tabs = shell.visible_tabs(cx);
            assert_eq!(tabs.len(), 5);
            assert_eq!(tabs[0], TitleTab::Workspace(first));
            for (tab, provider) in tabs[1..].iter().zip(providers) {
                assert_eq!(
                    shell.sessions[&tab.session().unwrap()]
                        .read(cx)
                        .summary
                        .provider,
                    provider.id()
                );
            }
            assert_eq!(shell.active_title_tab(cx).as_ref(), tabs.last());
        })
        .unwrap();
}

#[gpui::test]
fn project_round_trip_restores_the_selected_bot(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.bot_manager().update(cx, |bots, cx| {
                bots.create_named(vec!["/work/vorkspace".into()], window, cx);
                bots.dismiss_settings(window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let selected = window
        .update(cx, |shell, window, cx| {
            let selected = shell.active_title_tab(cx).unwrap();
            assert!(matches!(selected, TitleTab::Workspace(_)));
            shell.select_project("/work/another-project".into(), window, cx);
            selected
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, window, cx| {
            shell.select_project("/work/vorkspace".into(), window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.active_title_tab(cx), Some(selected.clone()));
            assert_eq!(
                shell.workspace.scope_focus.get(&shell.tab_scope()),
                Some(&selected)
            );
        })
        .unwrap();
}

#[gpui::test]
fn deleting_a_bot_prunes_its_saved_tabs(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.bot_manager().update(cx, |bots, cx| {
                bots.create_named(vec!["/work/vorkspace".into()], window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let selected = window
        .update(cx, |shell, _, cx| {
            let selected = shell.active_title_tab(cx).unwrap();
            assert!(matches!(selected, TitleTab::Workspace(_)));
            selected
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let delete = visual.debug_bounds("bot-delete").unwrap();
    visual.simulate_click(delete.center(), Default::default());
    visual.run_until_parked();
    visual.simulate_prompt_answer("Delete");
    visual.run_until_parked();
    window
        .update(visual, |shell, _, cx| {
            assert!(!shell.visible_tabs(cx).contains(&selected));
            assert!(!shell.workspace.title_order.contains(&selected));
            assert!(
                !shell
                    .workspace
                    .scope_focus
                    .values()
                    .any(|tab| tab == &selected)
            );
            let raw = locked(&shell.store).unwrap().ui_state().unwrap().unwrap();
            let saved: Workspace = serde_json::from_str(&raw).unwrap();
            assert!(!saved.title_order.contains(&selected));
            assert!(!saved.scope_focus.values().any(|tab| tab == &selected));
        })
        .unwrap();
}

#[gpui::test]
fn bot_tab_reconciliation_preserves_hidden_and_unavailable_rosters(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, window, cx| {
            shell.bot_manager().update(cx, |bots, cx| {
                bots.create_named(vec!["/work/vorkspace".into()], window, cx);
                bots.dismiss_settings(window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let selected = shell.active_title_tab(cx).unwrap();
            shell.workspace.preferences.bots = false;
            assert!(!shell.prune_deleted_bot_tabs(cx));
            assert!(shell.workspace.title_order.contains(&selected));
            let stale = shell.bot_tab("deleted-bot".into());
            shell.remember_title_tab(stale.clone());
            shell
                .bot_manager()
                .update(cx, |bots, _| bots.roster_loaded = false);
            assert!(!shell.prune_deleted_bot_tabs(cx));
            assert!(shell.workspace.title_order.contains(&stale));
            shell
                .bot_manager()
                .update(cx, |bots, _| bots.roster_loaded = true);
            assert!(shell.prune_deleted_bot_tabs(cx));
            assert!(shell.workspace.title_order.contains(&selected));
            assert!(!shell.workspace.title_order.contains(&stale));
            assert!(
                !shell
                    .workspace
                    .scope_focus
                    .values()
                    .any(|tab| tab == &stale)
            );
        })
        .unwrap();
}
