
use super::*;
use gpui::{
    AssetSource, Bounds, Render, TestAppContext, WindowBounds, WindowHandle, WindowOptions, point,
    size,
};
use std::sync::{Arc, Mutex};

struct Fixture {
    inbox: Entity<Inbox>,
    item: Item,
}
impl Render for Fixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.inbox.update(cx, |inbox, cx| {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(inbox.pr_actions(&self.item, cx))
                .child(inbox.summary_view(&self.item, window, cx))
        })
    }
}
fn fixture(cx: &mut TestAppContext) -> WindowHandle<Fixture> {
    let window = cx.update(|cx| cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(0.), px(0.)), size(px(1100.), px(700.))))),
            ..Default::default()
        }, |_, cx| cx.new(|cx| {
            let item = Item { provider: "github".into(), kind: "pr".into(), state: "open".into(), number: 7, ..Default::default() };
            let inbox = cx.new(|cx| {
                let store = Arc::new(Mutex::new(vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap()));
                let mut view = Inbox::new_inner(store, vec![], PathBuf::from("/private/tmp/inbox-regression"), false, cx);
                view.snapshot.items = vec![item.clone()]; view.selected = Some(item.key());
                view.details = Some(Details { body: "Description".into(), ..Default::default() });
                let comments = (0..100).map(|ix| model::Comment {
                    id: format!("comment-{ix}"), body: (0..30).map(|line| format!("### Finding {line}\n\nReview **retry handling** in `src/main.rs` before returning.\n\n")).collect(),
                    created_at: format!("2026-10-22T08:{:02}:00Z", ix % 60), ..Default::default()
                }).collect::<Vec<_>>();
                view.comments = comments.iter().map(|comment| (comment.clone(), cx.new(|cx| document::InboxDocument::new(comment.body.clone(), PathBuf::from("/repo"), cx)))).collect();
                view.thread = Some(Thread { comments, ..Default::default() });
                view.rebuild_activity(); view
            });
            Fixture { inbox, item }
        })).unwrap());
    draw(window, cx);
    window
}
fn draw(window: WindowHandle<Fixture>, cx: &mut TestAppContext) {
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
}
fn inbox_window(cx: &mut TestAppContext) -> WindowHandle<Inbox> {
    cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1100.), px(700.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let store = Arc::new(Mutex::new(
                        vorkspace_core::store::Store::open(std::path::Path::new(":memory:"))
                            .unwrap(),
                    ));
                    Inbox::new_inner(
                        store,
                        vec![],
                        PathBuf::from("/private/tmp/inbox-connectors"),
                        false,
                        cx,
                    )
                })
            },
        )
        .unwrap()
    })
}
#[gpui::test]
fn connector_pages_keep_drafts_filters_scroll_and_pending_details_with_one_observer(
    cx: &mut TestAppContext,
) {
    let window = inbox_window(cx);
    let (github, jira) = window
        .update(cx, |root, _, cx| {
            root.loading = true;
            root.snapshot.items = ["github", "jira"]
                .into_iter()
                .map(|provider| Item {
                    provider: provider.into(),
                    number: 42,
                    identifier: format!("{provider}-42"),
                    title: provider.into(),
                    updated_at: "2026-10-22T10:00:00Z".into(),
                    ..Default::default()
                })
                .collect();
            (
                root.connector_page("github", cx),
                root.connector_page("jira", cx),
            )
        })
        .unwrap();
    cx.update(|cx| {
        for (page, source, draft) in [
            (&github, "github", "GitHub draft"),
            (&jira, "jira", "Jira draft"),
        ] {
            page.update(cx, |page, cx| {
                assert_eq!(page.state.source, source);
                page.selected = Some(page.visible_items(cx)[0].key());
                page.detail_generation = 5;
                page.detail_loading = true;
                page.search
                    .update(cx, |input, cx| input.set_text(source.into(), cx));
                page.comment
                    .update(cx, |input, cx| input.set_text(draft.into(), cx));
                page.reply = Some((format!("{source}-thread"), source.into()));
                page.state.filters.time = if source == "github" { "7d" } else { "30d" }.into();
                page.summary_scroll = ListState::new(100, ListAlignment::Top, px(200.));
                page.summary_scroll.scroll_to(gpui::ListOffset {
                    item_ix: 12,
                    offset_in_item: px(4.),
                });
                page.set_active(true, cx);
                page.set_active(false, cx);
            });
        }
        github.update(cx, |page, cx| {
            page.finish_detail(
                4,
                page.selected.clone().unwrap(),
                Err("stale".into()),
                Err("stale".into()),
                cx,
            );
            assert!(page.detail_loading && page.detail_error.is_none());
            page.finish_detail(
                5,
                "jira:wrong-key".into(),
                Err("wrong provider".into()),
                Err("wrong provider".into()),
                cx,
            );
            assert!(page.detail_loading);
            page.finish_detail(
                5,
                page.selected.clone().unwrap(),
                Ok(Details {
                    body: "GitHub detail".into(),
                    ..Default::default()
                }),
                Ok(Thread::default()),
                cx,
            );
            assert_eq!(page.details.as_ref().unwrap().body, "GitHub detail");
        });
        jira.update(cx, |page, _| {
            assert!(page.details.is_none() && page.detail_loading)
        });
    });
    let changes = std::rc::Rc::new(std::cell::Cell::new(0));
    let output = changes.clone();
    let _events = window
        .update(cx, |root, _, cx| {
            let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Event, _| {
                if matches!(event, Event::Activity { .. }) {
                    output.set(output.get() + 1);
                }
            });
            let mut snapshot = root.snapshot.clone();
            snapshot.connections.insert(
                "jira".into(),
                model::Status {
                    connected: true,
                    ..Default::default()
                },
            );
            snapshot
                .errors
                .insert("github".into(), "Temporarily unavailable".into());
            snapshot.items.retain(|item| item.provider != "github");
            root.generation = 7;
            root.finish_refresh(
                7,
                root.state.fetch_scope(&root.projects),
                snapshot,
                vec![],
                cx,
            );
            subscription
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        changes.get(),
        1,
        "only the root observes and emits activity"
    );
    cx.update(|cx| {
        for (page, source, draft, time) in [
            (&github, "github", "GitHub draft", "7d"),
            (&jira, "jira", "Jira draft", "30d"),
        ] {
            page.update(cx, |page, cx| {
                page.set_active(true, cx);
                assert_eq!(page.state.source, source);
                assert_eq!(page.search.read(cx).content.as_ref(), source);
                assert_eq!(page.comment.read(cx).content.as_ref(), draft);
                assert_eq!(page.state.filters.time, time);
                assert_eq!(page.summary_scroll.logical_scroll_top().item_ix, 12);
                assert_eq!(page.reply.as_ref().unwrap().0, format!("{source}-thread"));
            });
        }
    });
    window
        .update(cx, |root, _, cx| {
            assert_eq!(root.pages.len(), 2);
            assert_eq!(root.connector_page("github", cx), github);
            let state = ViewState::load(&locked(&root.store).unwrap());
            assert_eq!(state.filters_for("github").time, "7d");
            assert_eq!(state.filters_for("jira").time, "30d");
        })
        .unwrap();
}
#[gpui::test]
fn retained_connector_pages_dispatch_with_current_defaults_and_preserve_explicit_destinations(
    cx: &mut TestAppContext,
) {
    let window = inbox_window(cx);
    let pages = window
        .update(cx, |root, _, cx| {
            root.loading = true;
            root.set_bots(
                ["first", "second", "chosen"]
                    .map(|id| (id.into(), id.into()))
                    .to_vec(),
                cx,
            );
            root.snapshot.items = ["jira", "gitlab"]
                .map(|provider| Item {
                    provider: provider.into(),
                    identifier: format!("{provider}-42"),
                    ..Default::default()
                })
                .to_vec();
            ["jira", "gitlab"].map(|provider| {
                root.set_default_bot(provider, "first", cx).unwrap();
                (provider, root.connector_page(provider, cx))
            })
        })
        .unwrap();
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _subscriptions = cx.update(|cx| {
        pages.each_ref().map(|(provider, page)| {
            page.update(cx, |page, cx| {
                // Retain a selected item without starting provider detail requests.
                page.selected = Some(
                    page.snapshot
                        .items
                        .iter()
                        .find(|item| item.provider == *provider)
                        .unwrap()
                        .key(),
                );
                page.reset_destination();
                assert_eq!(page.start_bot.as_deref(), Some("first"));
                page.comment
                    .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
                page.start_project = "/chosen/project".into();
                let output = sent.clone();
                cx.subscribe(&cx.entity(), move |_, _, event: &Event, _| {
                    if let Event::StartSession { item, bot, .. } = event {
                        output.borrow_mut().push((
                            item.provider.clone(),
                            bot.clone(),
                            item.project_path.clone(),
                        ));
                    }
                })
            })
        })
    });
    cx.run_until_parked();
    for explicit in [false, true] {
        if explicit {
            cx.update(|cx| {
                for (provider, page) in &pages {
                    page.update(cx, |page, _| {
                        page.destination_explicit = true;
                        page.start_bot = (*provider == "jira").then(|| "chosen".into());
                    });
                }
            });
        }
        for default in ["second", ""] {
            window
                .update(cx, |root, _, cx| {
                    for (provider, _) in &pages {
                        root.set_default_bot(provider, default, cx).unwrap();
                    }
                })
                .unwrap();
            cx.run_until_parked();
            cx.update(|cx| {
                for (provider, page) in &pages {
                    page.update(cx, |page, cx| {
                        let expected = if explicit {
                            (*provider == "jira").then_some("chosen")
                        } else {
                            (!default.is_empty()).then_some(default)
                        };
                        assert_eq!(page.start_bot.as_deref(), expected);
                        assert_eq!(
                            page.state.default_bots.get(provider),
                            (!default.is_empty()).then_some(default)
                        );
                        assert_eq!(page.comment.read(cx).content.as_ref(), "Unsent draft");
                        page.set_active(true, cx);
                        page.start(cx);
                        page.set_active(false, cx);
                    });
                }
            });
            cx.run_until_parked();
        }
    }
    let sent = sent.borrow();
    assert_eq!(sent.len(), 8);
    for (events, expected) in sent.as_chunks::<2>().0.iter().zip([
        [Some("second"), Some("second")],
        [None, None],
        [Some("chosen"), None],
        [Some("chosen"), None],
    ]) {
        for ((provider, bot, project), (source, expected)) in events
            .iter()
            .zip(["jira", "gitlab"].into_iter().zip(expected))
        {
            assert_eq!(provider, source);
            assert_eq!(bot.as_deref(), expected);
            if expected.is_none() {
                assert_eq!(project, "/chosen/project");
            }
        }
    }
}
#[gpui::test]
fn disabling_a_source_removes_controls_unread_and_selection_and_rejects_inflight_results(
    cx: &mut TestAppContext,
) {
    let window = inbox_window(cx);
    let items = model::PROVIDERS
        .into_iter()
        .map(|provider| Item {
            provider: provider.into(),
            kind: "issue".into(),
            number: 1,
            title: provider.into(),
            updated_at: "2026-10-22T10:00:00Z".into(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let old_scope = window
        .update(cx, |inbox, _, cx| {
            inbox.snapshot.items = items.clone();
            inbox.state.seen.seeded = true;
            for provider in model::PROVIDERS {
                inbox.snapshot.connections.insert(
                    provider.into(),
                    model::Status {
                        connected: true,
                        ..Default::default()
                    },
                );
            }
            inbox.selected = Some(items[0].key());
            inbox.details = Some(Details::default());
            inbox.loading = true;
            inbox.generation = 7;
            let old_scope = inbox.state.fetch_scope(&inbox.projects);
            inbox.set_connector_enabled("github", false, cx).unwrap();
            assert_eq!(inbox.unread_count(), 2);
            assert_eq!(inbox.state.source, "gitlab");
            assert!(inbox.selected.is_none() && inbox.details.is_none());
            assert!(!inbox.connection_statuses().contains_key("github"));
            assert_eq!(
                inbox
                    .visible_items(cx)
                    .iter()
                    .map(|item| item.provider.as_str())
                    .collect::<Vec<_>>(),
                ["gitlab"]
            );
            assert!(!model::Connectors::load(&locked(&inbox.store).unwrap()).enabled("github"));
            old_scope
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("inbox-source-github").is_none());
    assert!(visual.debug_bounds("inbox-source-gitlab").is_none());
    assert!(visual.debug_bounds("inbox-source-jira").is_none());
    window
        .update(visual, |inbox, _, cx| {
            inbox.set_connector_enabled("gitlab", false, cx).unwrap();
            inbox.set_connector_enabled("jira", false, cx).unwrap();
            assert!(inbox.state.source.is_empty());
            assert_eq!(inbox.unread_count(), 0);
            let stale = Snapshot {
                items,
                connections: [(
                    "github".into(),
                    model::Status {
                        connected: true,
                        ..Default::default()
                    },
                )]
                .into(),
                ..Default::default()
            };
            inbox.finish_refresh(7, old_scope, stale, vec![], cx);
            assert!(inbox.snapshot.items.is_empty());
            assert!(inbox.connection_statuses().is_empty());
            for provider in ["github", "linear", "azuredevops"] {
                inbox.open_connection(provider.into(), cx);
                assert!(inbox.connect_provider.is_none());
            }
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |inbox, _, _| {
            assert!(
                !inbox.loading,
                "the replacement all-disabled refresh must complete without adapters"
            );
            assert!(inbox.snapshot.items.is_empty() && inbox.snapshot.connections.is_empty());
        })
        .unwrap();
}
#[gpui::test]
fn own_successful_revision_is_read_suppressed_and_later_remote_revision_can_chime(
    cx: &mut TestAppContext,
) {
    let window = inbox_window(cx);
    let changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let output = changes.clone();
    let _subscription = window
        .update(cx, |inbox, _, cx| {
            let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Event, _| {
                if let Event::Activity { changed, .. } = event {
                    output.borrow_mut().push(changed.len());
                }
            });
            let mut item = Item {
                provider: "jira".into(),
                kind: "jira".into(),
                id: "42".into(),
                identifier: "ENG-42".into(),
                updated_at: "2026-10-22T10:00:00Z".into(),
                ..Default::default()
            };
            inbox.generation = 1;
            let scope = inbox.state.fetch_scope(&inbox.projects);
            inbox.finish_refresh(
                1,
                scope.clone(),
                Snapshot {
                    items: vec![item.clone()],
                    ..Default::default()
                },
                vec![],
                cx,
            );
            item.updated_at = "2026-10-22T11:00:00Z".into();
            inbox.record_self_activity(item.key(), item.updated_ms(), cx);
            inbox.finish_refresh(
                1,
                scope.clone(),
                Snapshot {
                    items: vec![item.clone()],
                    ..Default::default()
                },
                vec![],
                cx,
            );
            assert!(!inbox.state.seen.unseen(&item));
            assert!(
                inbox.self_activity.is_empty(),
                "coalesced own mutations are consumed by one remote revision"
            );
            item.updated_at = "2026-10-22T12:00:00Z".into();
            inbox.finish_refresh(
                1,
                scope,
                Snapshot {
                    items: vec![item],
                    ..Default::default()
                },
                vec![],
                cx,
            );
            subscription
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(*changes.borrow(), vec![0, 0, 1]);
}
#[gpui::test]
fn a_coalesced_external_revision_is_never_consumed_as_own_activity(cx: &mut TestAppContext) {
    let window = inbox_window(cx);
    let changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let output = changes.clone();
    let _subscription = window
        .update(cx, |inbox, _, cx| {
            let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Event, _| {
                if let Event::Activity { changed, .. } = event {
                    output.borrow_mut().push(changed.len());
                }
            });
            let mut item = Item {
                provider: "jira".into(),
                kind: "jira".into(),
                id: "42".into(),
                identifier: "ENG-42".into(),
                updated_at: "2026-10-22T10:00:00Z".into(),
                ..Default::default()
            };
            inbox.generation = 1;
            let scope = inbox.state.fetch_scope(&inbox.projects);
            inbox.finish_refresh(
                1,
                scope.clone(),
                Snapshot {
                    items: vec![item.clone()],
                    ..Default::default()
                },
                vec![],
                cx,
            );
            let own = model::timestamp_ms("2026-10-22T11:00:00Z");
            inbox.record_self_activity(item.key(), 0, cx);
            assert!(
                inbox.self_activity.is_empty(),
                "an uncertain receipt cannot suppress anything"
            );
            inbox.record_self_activity(item.key(), own, cx);
            inbox.record_self_activity(item.key(), own - 1, cx);
            assert_eq!(
                inbox.self_activity[&item.key()].0,
                own,
                "older completions cannot overwrite the newer receipt"
            );
            // A poll started before the write can still return the old snapshot.
            inbox.finish_refresh(
                1,
                scope.clone(),
                Snapshot {
                    items: vec![item.clone()],
                    ..Default::default()
                },
                vec![],
                cx,
            );
            assert_eq!(inbox.self_activity[&item.key()].0, own);
            // The own revision is never polled: another person's update arrives first.
            item.updated_at = "2026-10-22T12:00:00Z".into();
            inbox.finish_refresh(
                1,
                scope,
                Snapshot {
                    items: vec![item.clone()],
                    ..Default::default()
                },
                vec![],
                cx,
            );
            assert!(inbox.state.seen.unseen(&item));
            assert!(inbox.self_activity.is_empty());
            subscription
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(*changes.borrow(), vec![0, 0, 1]);
}
#[gpui::test]
fn saved_bot_destination_is_preselected_and_explicit_project_choice_wins(cx: &mut TestAppContext) {
    let window = inbox_window(cx);
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let output = sent.clone();
    let _subscription = window
        .update(cx, |inbox, _, cx| {
            let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Event, _| {
                if let Event::StartSession { item, prompt, bot } = event {
                    output.borrow_mut().push((
                        item.project_path.clone(),
                        prompt.clone(),
                        bot.clone(),
                    ));
                }
            });
            let item = Item {
                provider: "jira".into(),
                kind: "jira".into(),
                identifier: "ENG-42".into(),
                title: "Preserve context".into(),
                ..Default::default()
            };
            inbox.state.source = "jira".into();
            inbox.snapshot.items = vec![item.clone()];
            inbox.selected = Some(item.key());
            inbox.details = Some(Details {
                body: "Ticket description".into(),
                ..Default::default()
            });
            inbox.set_bots(vec![("helper".into(), "Helper".into())], cx);
            inbox.set_default_bot("jira", "helper", cx).unwrap();
            assert_eq!(inbox.start_bot.as_deref(), Some("helper"));
            inbox.start(cx);
            inbox.start_project = "/chosen/project".into();
            inbox.start_bot = None;
            inbox.destination_explicit = true;
            inbox.set_default_bot("jira", "helper", cx).unwrap();
            assert!(
                inbox.start_bot.is_none(),
                "saved defaults cannot override a deliberate destination"
            );
            inbox.start(cx);
            inbox.destination_explicit = false;
            inbox.set_bots(vec![], cx);
            assert!(inbox.start_bot.is_none());
            assert_eq!(inbox.state.default_bots.get("jira"), Some("helper"));
            subscription
        })
        .unwrap();
    cx.run_until_parked();
    let sent = sent.borrow();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].2.as_deref(), Some("helper"));
    assert!(sent[0].1.contains("ENG-42") && sent[0].1.contains("Ticket description"));
    assert_eq!(sent[1].0, "/chosen/project");
    assert_eq!(sent[1].2, None);
}
#[gpui::test]
fn long_discussion_materializes_only_viewport_comments_and_reply_focus_reaches_composer(
    cx: &mut TestAppContext,
) {
    let window = fixture(cx);
    window
        .update(cx, |fixture, _, cx| {
            fixture.inbox.update(cx, |inbox, cx| {
                let measured = inbox
                    .comments
                    .iter()
                    .filter(|(_, doc)| doc.read(cx).measured_rows() > 0)
                    .count();
                assert!(
                    measured > 0 && measured < 10,
                    "only viewport/overscan cards should be laid out, got {measured}"
                );
                assert_eq!(inbox.summary_scroll.item_count(), 103);
                inbox.summary_scroll.scroll_by(px(2500.));
            })
        })
        .unwrap();
    draw(window, cx);
    window
        .update(cx, |fixture, window, cx| {
            fixture.inbox.update(cx, |inbox, cx| {
                assert!(inbox.summary_scroll.logical_scroll_top().item_ix > 3);
                inbox.reply = Some(("thread-1".into(), "Reviewer".into()));
                inbox.summary_scroll.remeasure_items(102..103);
                inbox.summary_scroll.scroll_to_end();
                window.focus(&inbox.comment.focus_handle(cx), cx);
            })
        })
        .unwrap();
    draw(window, cx);
    window
        .update(cx, |fixture, window, cx| {
            fixture.inbox.update(cx, |inbox, cx| {
                assert!(inbox.summary_scroll.logical_scroll_top().item_ix > 90);
                assert!(inbox.comment.focus_handle(cx).is_focused(window));
                inbox
                    .comment
                    .update(cx, |input, cx| input.set_text("A reply\n".repeat(8), cx));
            })
        })
        .unwrap();
    draw(window, cx);
    window
        .update(cx, |fixture, window, cx| {
            fixture.inbox.update(cx, |inbox, cx| {
                assert!(inbox.comment.focus_handle(cx).is_focused(window));
                assert_eq!(inbox.summary_scroll.item_count(), 103);
            })
        })
        .unwrap();
}
#[gpui::test]
fn expanding_comment_invalidates_its_height_and_preserves_timeline_order(cx: &mut TestAppContext) {
    let window = fixture(cx);
    let (id, previous) = window
        .update(cx, |fixture, _, cx| {
            fixture.inbox.update(cx, |inbox, cx| {
                let Activity::Comment(comment) = &inbox.activity[0] else {
                    panic!()
                };
                let id = comment.id.clone();
                let doc = &inbox
                    .comments
                    .iter()
                    .find(|(comment, _)| comment.id == id)
                    .unwrap()
                    .1;
                let previous = doc.read(cx).measured_rows();
                inbox.expanded_comments.insert(id.clone());
                inbox.summary_scroll.remeasure();
                (id, previous)
            })
        })
        .unwrap();
    draw(window, cx);
    window
        .update(cx, |fixture, _, cx| {
            fixture.inbox.update(cx, |inbox, cx| {
                let doc = &inbox
                    .comments
                    .iter()
                    .find(|(comment, _)| comment.id == id)
                    .unwrap()
                    .1;
                assert!(doc.read(cx).measured_rows() > previous);
                let Activity::Comment(first) = &inbox.activity[0] else {
                    panic!()
                };
                assert_eq!(first.id, id);
                assert_eq!(inbox.summary_scroll.logical_scroll_top().item_ix, 0);
            })
        })
        .unwrap();
}
#[gpui::test]
fn arriving_comment_preserves_the_readers_item_and_offset(cx: &mut TestAppContext) {
    let window = fixture(cx);
    window
        .update(cx, |fixture, _, cx| {
            fixture
                .inbox
                .update(cx, |inbox, _| inbox.summary_scroll.scroll_by(px(2500.)))
        })
        .unwrap();
    draw(window, cx);
    let before = window
        .update(cx, |fixture, _, cx| {
            fixture.inbox.update(cx, |inbox, cx| {
                let before = inbox.summary_scroll.logical_scroll_top();
                let comment = model::Comment {
                    id: "new-comment".into(),
                    body: "A new reply".into(),
                    created_at: "2026-10-22T09:00:00Z".into(),
                    ..Default::default()
                };
                let doc = cx.new(|cx| {
                    document::InboxDocument::new(comment.body.clone(), PathBuf::from("/repo"), cx)
                });
                inbox.comments.push((comment.clone(), doc));
                inbox.thread.as_mut().unwrap().comments.push(comment);
                inbox.rebuild_activity();
                before
            })
        })
        .unwrap();
    draw(window, cx);
    window
        .update(cx, |fixture, _, cx| {
            fixture.inbox.update(cx, |inbox, _| {
                let after = inbox.summary_scroll.logical_scroll_top();
                assert_eq!(after.item_ix, before.item_ix);
                assert!((after.offset_in_item - before.offset_in_item).abs() < px(1.));
                assert_eq!(inbox.summary_scroll.item_count(), 104);
            })
        })
        .unwrap();
}

#[test]
fn toolbar_icons_have_explicit_ink_and_the_original_distinct_assets() {
    for (name, ink) in [
        ("git-merge", shell_fill()),
        ("git-pull-request-draft", content(0.7)),
        ("git-pull-request-closed", content(0.7)),
        ("external-link", content(0.7)),
    ] {
        let mut icon = action_icon(name, ink);
        assert_eq!(icon.style().text.color, Some(ink));
        let bytes = crate::assets::Assets.load(name).unwrap().unwrap();
        assert!(std::str::from_utf8(&bytes).unwrap().contains("<svg"));
    }
}
