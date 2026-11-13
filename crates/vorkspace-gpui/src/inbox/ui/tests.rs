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
struct PinnedFixture {
    _root: Entity<Inbox>,
    page: Entity<Inbox>,
}
struct TiledFixture {
    inbox: Entity<Inbox>,
    width: f32,
}
impl Render for TiledFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("connector-tile")
                .debug_selector(|| "connector-tile".into())
                .w(px(self.width))
                .h_full()
                .child(self.inbox.clone()),
        )
    }
}
impl Render for PinnedFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.page.clone())
    }
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
                let store = Arc::new(Mutex::new(vorkspace_core::store::Store::test(std::path::Path::new(":memory:")).unwrap()));
                let mut view = Inbox::new_for_test(store, vec![], PathBuf::from("/private/tmp/inbox-regression"), cx);
                Rc::make_mut(&mut view.snapshot).items = vec![item.clone()]; view.selected = Some(item.key());
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
    inbox_window_at_width(cx, 1100.)
}
fn inbox_window_at_width(cx: &mut TestAppContext, width: f32) -> WindowHandle<Inbox> {
    cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(width), px(700.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let store = Arc::new(Mutex::new(
                        vorkspace_core::store::Store::test(std::path::Path::new(":memory:"))
                            .unwrap(),
                    ));
                    Inbox::new_for_test(
                        store,
                        vec![],
                        PathBuf::from("/private/tmp/inbox-connectors"),
                        cx,
                    )
                })
            },
        )
        .unwrap()
    })
}

#[gpui::test]
fn connector_detail_headers_start_at_the_pane_inset_at_every_zoom(cx: &mut TestAppContext) {
    let window = inbox_window_at_width(cx, 1920.);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    for (provider, kind) in [("github", "pr"), ("gitlab", "pr"), ("jira", "issue")] {
        window
            .update(visual, |inbox, _, cx| {
                let item = Item {
                    provider: provider.into(),
                    kind: kind.into(),
                    number: 137,
                    repo: "example/project".into(),
                    title: "Connector detail alignment".into(),
                    state: "open".into(),
                    ..Default::default()
                };
                inbox.state.source = provider.into();
                inbox.selected = Some(item.detail_key());
                Rc::make_mut(&mut inbox.snapshot).items = vec![item];
                inbox.details = Some(Details::default());
                inbox.narrow_detail = true;
                cx.notify();
            })
            .unwrap();

        for (width, zoom) in [(1920., 1.), (1600., 0.85), (860., 1.3)] {
            visual.simulate_resize(size(px(width), px(700.)));
            window
                .update(visual, |_, window, cx| window.set_ui_scale(zoom, cx))
                .unwrap();
            visual.run_until_parked();
            visual.refresh().unwrap();
            let header = visual.debug_bounds("inbox-detail-header").unwrap();
            let meta = visual.debug_bounds("inbox-detail-meta").unwrap();
            let inset = window
                .update(visual, |inbox, window, _| inbox.detail_inset(window))
                .unwrap();
            let offset = meta.left() - header.left();
            assert!(
                (offset - inset).abs() <= px(1. / zoom),
                "{provider} at {width}px and {zoom}×: {offset:?} instead of {inset:?}"
            );
        }
    }
}

#[gpui::test]
fn narrow_navigation_preserves_the_open_item_and_comment_draft(cx: &mut TestAppContext) {
    let window = inbox_window_at_width(cx, 720.);
    let key = window
        .update(cx, |inbox, _, cx| {
            let item = Item {
                provider: "github".into(),
                kind: "issue".into(),
                number: 42,
                title: "Draft fixture".into(),
                state: "open".into(),
                ..Default::default()
            };
            let key = item.key();
            Rc::make_mut(&mut inbox.snapshot).items = vec![item];
            inbox.selected = Some(key.clone());
            inbox.details = Some(Details::default());
            inbox.narrow_detail = true;
            inbox
                .comment
                .update(cx, |input, cx| input.set_text("Unsent comment".into(), cx));
            cx.notify();
            key
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("connector-list").is_none());
    let back = visual.debug_bounds("inbox-back").unwrap().center();
    visual.simulate_click(back, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("connector-list").is_some());
    assert!(visual.debug_bounds("inbox-back").is_none());
    let selector = Box::leak(format!("inbox-item-{key}").into_boxed_str());
    let row = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(row, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("inbox-back").is_some());
    window
        .update(visual, |inbox, _, cx| {
            assert_eq!(inbox.selected.as_deref(), Some(key.as_str()));
            assert_eq!(inbox.comment.read(cx).content.as_ref(), "Unsent comment");
            assert!(inbox.details.is_some());
        })
        .unwrap();
}

#[gpui::test]
fn pinned_connector_tabs_retain_server_identity_and_draft_after_reconnect(cx: &mut TestAppContext) {
    let window = inbox_window_at_width(cx, 1100.);
    let old = Item {
        provider: "jira".into(),
        kind: "issue".into(),
        id: "10042".into(),
        identifier: "DEMO-42".into(),
        title: "Previous site issue".into(),
        url: "https://previous.example/browse/DEMO-42".into(),
        ..Default::default()
    };
    let new = Item {
        title: "Current site issue".into(),
        url: "https://current.example/browse/DEMO-42".into(),
        updated_at: "2026-11-10T10:00:00Z".into(),
        ..old.clone()
    };
    assert_eq!(old.key(), new.key());
    let (previous, current) = window
        .update(cx, |root, _, cx| {
            let previous = root.connector_item_page("jira", old.clone(), cx);
            previous.update(cx, |page, cx| {
                page.comment.update(cx, |input, cx| {
                    input.set_text("Draft for the previous site".into(), cx);
                });
            });
            Rc::make_mut(&mut root.snapshot).items = vec![new.clone()];
            let current = root.connector_item_page("jira", new.clone(), cx);
            assert_ne!(previous, current);
            assert!(
                !crate::workspace::AppPane::ConnectorItem(Box::new(old.clone())).same_resource(
                    &crate::workspace::AppPane::ConnectorItem(Box::new(new.clone()))
                )
            );
            root.publish_pages(cx);
            (previous, current)
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let previous = previous.read(cx);
        assert_eq!(previous.pinned_item(), Some(&old));
        assert_eq!(
            previous.comment.read(cx).content.as_ref(),
            "Draft for the previous site"
        );
        assert_eq!(current.read(cx).pinned_item(), Some(&new));
    });
}

#[gpui::test]
fn a_narrow_connector_tile_uses_its_width_inside_a_wide_window(cx: &mut TestAppContext) {
    let width = 420.;
    let item = Item {
        provider: "github".into(),
        kind: "issue".into(),
        number: 42,
        title: "Tile width fixture".into(),
        state: "open".into(),
        ..Default::default()
    };
    let key = item.key();
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1440.), px(800.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let store = Arc::new(Mutex::new(
                        vorkspace_core::store::Store::test(std::path::Path::new(":memory:"))
                            .unwrap(),
                    ));
                    let inbox = cx.new(|cx| {
                        let mut inbox = Inbox::new_for_test(
                            store,
                            vec![],
                            PathBuf::from("/private/tmp/inbox-tile"),
                            cx,
                        );
                        Rc::make_mut(&mut inbox.snapshot).items = vec![item];
                        inbox.selected = Some(key.clone());
                        inbox.details = Some(Details::default());
                        inbox
                    });
                    TiledFixture { inbox, width }
                })
            },
        )
        .unwrap()
    });
    window
        .update(cx, |fixture, window, cx| {
            assert!(!page::narrow(window));
            fixture
                .inbox
                .update(cx, |inbox, cx| inbox.set_pane_width(width, cx));
            assert_eq!(fixture.inbox.read(cx).detail_inset(window), px(16.));
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let tile = visual.debug_bounds("connector-tile").unwrap();
    let list = visual.debug_bounds("connector-list").unwrap();
    assert!((f32::from(tile.size.width) - width).abs() < 1.);
    assert!((f32::from(list.size.width) - width).abs() < 1.);
    assert!(visual.debug_bounds("inbox-back").is_none());
    let row = Box::leak(format!("inbox-item-{key}").into_boxed_str());
    let row_center = visual.debug_bounds(row).unwrap().center();
    visual.simulate_click(row_center, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("connector-list").is_none());
    let back = visual.debug_bounds("inbox-back").unwrap();
    assert!(back.center().x >= tile.origin.x && back.center().x <= tile.origin.x + tile.size.width);
}

#[gpui::test]
fn connector_pages_keep_drafts_filters_scroll_and_pending_details_with_one_observer(
    cx: &mut TestAppContext,
) {
    let window = inbox_window(cx);
    let (github, jira) = window
        .update(cx, |root, _, cx| {
            root.loading = true;
            Rc::make_mut(&mut root.snapshot).items = ["github", "jira"]
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
                page.selected = Some(page.first_visible_key(cx).unwrap());
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
            let subscription = cx.subscribe(&root.feed, move |_, _, event: &FeedEvent, _| {
                if matches!(event, FeedEvent::Discovered { .. }) {
                    output.set(output.get() + 1);
                }
            });
            let mut snapshot = (*root.snapshot).clone();
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
            let generation = root.stage_fixture_poll(cx);
            root.finish_fixture_poll(generation, snapshot, cx);
            subscription
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        changes.get(),
        1,
        "the shared feed emits once for both provider pages"
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
            for page in [&github, &jira] {
                assert!(Rc::ptr_eq(&root.snapshot, &page.read(cx).snapshot));
                assert!(Rc::ptr_eq(&root.sessions, &page.read(cx).sessions));
            }
            let state = ViewState::load(&locked(&root.store).unwrap());
            assert_eq!(state.filters_for("github").time, "7d");
            assert_eq!(state.filters_for("jira").time, "30d");
        })
        .unwrap();
}

#[gpui::test]
fn item_pages_keep_pinned_identity_and_independent_view_state_across_snapshots(
    cx: &mut TestAppContext,
) {
    let window = inbox_window(cx);
    let first = Item {
        provider: "github".into(),
        kind: "issue".into(),
        number: 41,
        repo: "team/repo".into(),
        title: "First issue".into(),
        updated_at: "2026-11-01T10:00:00Z".into(),
        project_path: "/work/clone-b".into(),
        ..Default::default()
    };
    let second = Item {
        number: 42,
        title: "Second issue".into(),
        ..first.clone()
    };
    let (section, first_page, second_page) = window
        .update(cx, |root, _, cx| {
            Rc::make_mut(&mut root.snapshot).items = vec![
                Item {
                    project_path: "/work/clone-a".into(),
                    other_project_paths: vec![first.project_path.clone()],
                    ..first.clone()
                },
                second.clone(),
            ];
            assert_eq!(
                root.item_by_key_for_project(&first.key(), &first.project_path)
                    .unwrap()
                    .project_path,
                first.project_path
            );
            let section = root.connector_page("github", cx);
            let first_page = root.connector_item_page("github", first.clone(), cx);
            let second_page = root.connector_item_page("github", second.clone(), cx);
            assert_eq!(
                root.connector_item_page("github", first.clone(), cx),
                first_page
            );
            assert_eq!(root.pages.len(), 3);
            (section, first_page, second_page)
        })
        .unwrap();
    cx.update(|cx| {
        section.update(cx, |page, _| {
            page.selected = Some(second.key());
        });
        for (page, item, draft) in [
            (&first_page, &first, "First draft"),
            (&second_page, &second, "Second draft"),
        ] {
            page.update(cx, |page, cx| {
                assert_eq!(page.pinned_item().unwrap().key(), item.key());
                assert_eq!(page.document_cwd(), PathBuf::from(&item.project_path));
                page.comment
                    .update(cx, |input, cx| input.set_text(draft.into(), cx));
                page.reply = Some((item.key(), draft.into()));
                page.summary_scroll = ListState::new(100, ListAlignment::Top, px(200.));
                page.summary_scroll.scroll_to(gpui::ListOffset {
                    item_ix: if item.number == 41 { 12 } else { 24 },
                    offset_in_item: px(4.),
                });
                page.detail_generation = 4;
                page.detail_loading = true;
                page.finish_detail(
                    4,
                    item.key(),
                    Ok(Details {
                        body: draft.into(),
                        ..Default::default()
                    }),
                    Ok(Thread::default()),
                    cx,
                );
            });
        }
    });
    window
        .update(cx, |root, _, cx| {
            Rc::make_mut(&mut root.snapshot).items = vec![Item {
                title: "Second issue updated".into(),
                updated_at: "2026-11-02T10:00:00Z".into(),
                ..second.clone()
            }];
            root.publish_pages(cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        first_page.update(cx, |page, cx| {
            assert_eq!(page.item().unwrap().key(), first.key());
            assert_eq!(page.pane_title(), "First issue");
            assert_eq!(page.details.as_ref().unwrap().body, "First draft");
            assert_eq!(page.comment.read(cx).content.as_ref(), "First draft");
            assert_eq!(page.reply.as_ref().unwrap().0, first.key());
            assert_eq!(page.summary_scroll.logical_scroll_top().item_ix, 12);
            page.finish_detail(
                4,
                second.key(),
                Err("wrong item".into()),
                Err("wrong item".into()),
                cx,
            );
            assert_eq!(page.details.as_ref().unwrap().body, "First draft");
        });
        second_page.update(cx, |page, cx| {
            assert_eq!(page.item().unwrap().key(), second.key());
            assert_eq!(page.pane_title(), "Second issue updated");
            assert_eq!(page.details.as_ref().unwrap().body, "Second draft");
            assert_eq!(page.comment.read(cx).content.as_ref(), "Second draft");
            assert_eq!(page.summary_scroll.logical_scroll_top().item_ix, 24);
        });
        section.update(cx, |page, _| {
            assert!(page.pinned_item().is_none());
            assert_eq!(page.selected.as_deref(), Some(second.key().as_str()));
            assert_eq!(page.pane_title(), "GitHub");
        });
    });
    window
        .update(cx, |root, _, cx| {
            Rc::make_mut(&mut root.snapshot).items = vec![Item {
                title: "First issue updated".into(),
                updated_at: "2026-11-03T10:00:00Z".into(),
                project_path: "/work/clone-a".into(),
                other_project_paths: vec![first.project_path.clone()],
                ..first.clone()
            }];
            root.publish_pages(cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        first_page.update(cx, |page, _| {
            assert_eq!(page.item().unwrap().key(), first.key());
            assert_eq!(page.pane_title(), "First issue updated");
            assert_eq!(page.document_cwd(), PathBuf::from(&first.project_path));
        });
        second_page.update(cx, |page, _| {
            assert_eq!(page.item().unwrap().key(), second.key());
            assert_eq!(page.pane_title(), "Second issue updated");
        });
    });
}

#[gpui::test]
fn item_page_read_receipts_do_not_restore_old_filters_and_closed_pages_are_released(
    cx: &mut TestAppContext,
) {
    let window = inbox_window(cx);
    let item = Item {
        provider: "jira".into(),
        id: "10041".into(),
        identifier: "TEAM-41".into(),
        title: "Pinned issue".into(),
        ..Default::default()
    };
    let page = window
        .update(cx, |root, _, cx| {
            Rc::make_mut(&mut root.snapshot).items.push(item.clone());
            root.connector_item_page("jira", item.clone(), cx)
        })
        .unwrap();
    window
        .update(cx, |root, _, _| {
            root.state
                .provider_filters
                .entry("jira".into())
                .or_default()
                .time = "7d".into();
            root.state.list_width = 340.;
        })
        .unwrap();
    page.update(cx, |page, cx| {
        page.state.seen.mark(&item);
        page.save(cx);
    });
    window
        .update(cx, |root, _, cx| {
            assert_eq!(root.state.provider_filters["jira"].time, "7d");
            assert_eq!(root.state.list_width, 340.);
            root.release_item_page(&item, cx);
            assert!(root.pages.is_empty());
        })
        .unwrap();
    let retained = window
        .update(cx, |root, _, cx| {
            root.connector_item_page("jira", item.clone(), cx)
        })
        .unwrap();
    retained.update(cx, |page, _| page.posting = true);
    window
        .update(cx, |root, _, cx| {
            root.release_item_page(&item, cx);
            assert_eq!(root.pages.len(), 1, "keep in-flight writes observable");
            assert_eq!(root.connector_item_page("jira", item.clone(), cx), retained);
        })
        .unwrap();
    retained.update(cx, |page, cx| {
        page.posting = false;
        page.finish_pending_release(cx);
    });
    window
        .update(cx, |root, _, cx| {
            assert_eq!(root.pages.len(), 1, "reopening cancels pending release");
            root.release_item_page(&item, cx);
            assert!(root.pages.is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn item_tab_actions_keep_ordinary_selection_and_pinned_page_has_no_list(cx: &mut TestAppContext) {
    let window = inbox_window(cx);
    let first = Item {
        provider: "github".into(),
        kind: "issue".into(),
        number: 51,
        repo: "team/repo".into(),
        title: "First issue".into(),
        ..Default::default()
    };
    let second = Item {
        number: 52,
        title: "Second issue".into(),
        ..first.clone()
    };
    let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _subscription = window
        .update(cx, |root, _, cx| {
            Rc::make_mut(&mut root.snapshot).items = vec![first.clone(), second.clone()];
            root.selected = Some(first.key());
            root.details = Some(Details::default());
            let opened = opened.clone();
            let subscription = cx.subscribe(&cx.entity(), move |_, _, event: &Event, _| {
                if let Event::OpenTab(item) = event {
                    opened.borrow_mut().push(item.key());
                }
            });
            cx.notify();
            subscription
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let header = visual.debug_bounds("inbox-open-tab").unwrap().center();
    visual.simulate_click(header, Default::default());
    let selector = Box::leak(format!("inbox-item-{}", second.key()).into_boxed_str());
    let row = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(
        row,
        gpui::Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.simulate_mouse_down(row, MouseButton::Middle, Default::default());
    visual.simulate_mouse_up(row, MouseButton::Middle, Default::default());
    assert_eq!(
        opened.borrow().as_slice(),
        &[first.key(), second.key(), second.key()]
    );
    window
        .update(visual, |root, _, _| {
            assert_eq!(root.selected.as_deref(), Some(first.key().as_str()));
        })
        .unwrap();
    visual.simulate_click(row, Default::default());
    window
        .update(visual, |root, _, _| {
            assert_eq!(root.selected.as_deref(), Some(second.key().as_str()));
        })
        .unwrap();
}

#[gpui::test]
fn pinned_item_page_renders_detail_without_a_library_list(cx: &mut TestAppContext) {
    let item = Item {
        provider: "github".into(),
        kind: "issue".into(),
        number: 51,
        repo: "team/repo".into(),
        title: "Pinned issue".into(),
        ..Default::default()
    };
    let pinned = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(720.), px(700.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                let store = Arc::new(Mutex::new(
                    vorkspace_core::store::Store::test(std::path::Path::new(":memory:")).unwrap(),
                ));
                let root = cx.new(|cx| {
                    let mut root = Inbox::new_for_test(
                        store,
                        vec![],
                        PathBuf::from("/private/tmp/inbox-pinned"),
                        cx,
                    );
                    Rc::make_mut(&mut root.snapshot).items = vec![item.clone()];
                    root
                });
                let page = root.update(cx, |root, cx| {
                    root.connector_item_page("github", item.clone(), cx)
                });
                cx.new(|_| PinnedFixture { _root: root, page })
            },
        )
        .unwrap()
    });
    let pinned_visual = &mut gpui::VisualTestContext::from_window(pinned.into(), cx);
    pinned_visual.run_until_parked();
    pinned_visual.refresh().unwrap();
    assert!(pinned_visual.debug_bounds("connector-list").is_none());
    assert!(pinned_visual.debug_bounds("inbox-open-tab").is_none());
    assert!(pinned_visual.debug_bounds("inbox-open-external").is_some());
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
            Rc::make_mut(&mut root.snapshot).items = ["jira", "gitlab"]
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
                        .detail_key(),
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
    let generation = window
        .update(cx, |inbox, _, cx| {
            Rc::make_mut(&mut inbox.snapshot).items = items.clone();
            inbox.state.seen.seeded = true;
            for provider in model::PROVIDERS {
                Rc::make_mut(&mut inbox.snapshot).connections.insert(
                    provider.into(),
                    model::Status {
                        connected: true,
                        ..Default::default()
                    },
                );
            }
            inbox.selected = Some(items[0].key());
            inbox.details = Some(Details::default());
            let generation = inbox.stage_fixture_poll(cx);
            inbox.set_connector_enabled("github", false, cx).unwrap();
            assert_eq!(inbox.unread_count(), 2);
            assert_eq!(inbox.state.source, "gitlab");
            assert!(inbox.selected.is_none() && inbox.details.is_none());
            assert!(!inbox.connection_statuses().contains_key("github"));
            assert_eq!(
                inbox
                    .visible_items(cx)
                    .map(|item| item.provider.as_str())
                    .collect::<Vec<_>>(),
                ["gitlab"]
            );
            assert!(!model::Connectors::load(&locked(&inbox.store).unwrap()).enabled("github"));
            generation
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
            inbox.finish_fixture_poll(generation, stale, cx);
            assert!(inbox.snapshot.items.is_empty());
            assert!(inbox.connection_statuses().is_empty());
            for provider in ["github", "linear", "azuredevops"] {
                inbox.open_connection(provider.into(), cx);
                assert!(inbox.connect.provider.is_none());
            }
        })
        .unwrap();
    visual.run_until_parked();
    window
        .update(visual, |inbox, _, _| {
            assert!(
                !inbox.loading,
                "the stale poll must release its loading state with all connectors disabled"
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
            let subscription = cx.subscribe(&inbox.feed, move |_, _, event: &FeedEvent, _| {
                if let FeedEvent::Discovered { changed, .. } = event {
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
            let generation = inbox.stage_fixture_poll(cx);
            inbox.finish_fixture_poll(
                generation,
                Snapshot {
                    items: vec![item.clone()],
                    ..Default::default()
                },
                cx,
            );
            item.updated_at = "2026-10-22T11:00:00Z".into();
            inbox.record_self_activity(item.detail_key(), item.updated_ms(), cx);
            let generation = inbox.stage_fixture_poll(cx);
            inbox.finish_fixture_poll(
                generation,
                Snapshot {
                    items: vec![item.clone()],
                    ..Default::default()
                },
                cx,
            );
            assert!(!inbox.state.seen.unseen(&item));
            assert!(
                inbox
                    .feed
                    .read(cx)
                    .mutation_revision(&item.detail_key())
                    .is_none(),
                "coalesced own mutations are consumed by one remote revision"
            );
            item.updated_at = "2026-10-22T12:00:00Z".into();
            let generation = inbox.stage_fixture_poll(cx);
            inbox.finish_fixture_poll(
                generation,
                Snapshot {
                    items: vec![item],
                    ..Default::default()
                },
                cx,
            );
            subscription
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(*changes.borrow(), vec![0, 0, 1]);
}

#[gpui::test]
fn a_comment_receipt_only_clears_the_submitted_draft(cx: &mut TestAppContext) {
    let window = inbox_window(cx);
    window
        .update(cx, |inbox, _, cx| {
            inbox.selected = Some("selected-item".into());
            inbox.loading = true;
            inbox.release_pending = true;
            inbox
                .comment
                .update(cx, |input, cx| input.set_text("First comment".into(), cx));
        })
        .unwrap();
    let posted = window
        .update(cx, |inbox, _, cx| {
            let posted = PostedComment {
                key: inbox.selected.clone().unwrap(),
                revision: inbox.comment_revision,
                reply: inbox.reply.clone(),
            };
            inbox
                .comment
                .update(cx, |input, cx| input.set_text("New draft".into(), cx));
            posted
        })
        .unwrap();
    let receipt = || {
        Ok(model::CommentReceipt {
            url: "https://example.test/comment".into(),
            revision: None,
        })
    };
    window
        .update(cx, |inbox, _, cx| {
            inbox.finish_comment(posted, receipt(), cx);
            assert_eq!(inbox.comment.read(cx).content.as_ref(), "New draft");
            inbox.finish_comment(
                PostedComment {
                    key: inbox.selected.clone().unwrap(),
                    revision: inbox.comment_revision,
                    reply: inbox.reply.clone(),
                },
                receipt(),
                cx,
            );
            assert!(inbox.comment.read(cx).content.is_empty());
        })
        .unwrap();
}
#[gpui::test]
fn a_coalesced_external_revision_is_never_consumed_as_own_activity(cx: &mut TestAppContext) {
    let window = inbox_window(cx);
    let changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let output = changes.clone();
    let _subscription = window
        .update(cx, |inbox, _, cx| {
            let subscription = cx.subscribe(&inbox.feed, move |_, _, event: &FeedEvent, _| {
                if let FeedEvent::Discovered { changed, .. } = event {
                    output.borrow_mut().push(changed.len());
                }
            });
            let mut item = Item {
                provider: "jira".into(),
                kind: "jira".into(),
                id: "42".into(),
                identifier: "ENG-42".into(),
                url: "https://current.example.test/browse/ENG-42".into(),
                updated_at: "2026-10-22T10:00:00Z".into(),
                ..Default::default()
            };
            let generation = inbox.stage_fixture_poll(cx);
            inbox.finish_fixture_poll(
                generation,
                Snapshot {
                    items: vec![item.clone()],
                    ..Default::default()
                },
                cx,
            );
            let own = model::timestamp_ms("2026-10-22T11:00:00Z");
            let mut previous = item.clone();
            previous.url = "https://previous.example.test/browse/ENG-42".into();
            inbox.record_self_activity(previous.detail_key(), own, cx);
            inbox.record_self_activity(item.detail_key(), 0, cx);
            assert!(
                inbox
                    .feed
                    .read(cx)
                    .mutation_revision(&item.detail_key())
                    .is_none(),
                "an uncertain receipt cannot suppress anything"
            );
            inbox.record_self_activity(item.detail_key(), own, cx);
            inbox.record_self_activity(item.detail_key(), own - 1, cx);
            assert_eq!(
                inbox.feed.read(cx).mutation_revision(&item.detail_key()),
                Some(own),
                "older completions cannot overwrite the newer receipt"
            );
            // A poll started before the write can still return the old snapshot.
            let generation = inbox.stage_fixture_poll(cx);
            inbox.finish_fixture_poll(
                generation,
                Snapshot {
                    items: vec![item.clone()],
                    ..Default::default()
                },
                cx,
            );
            assert_eq!(
                inbox.feed.read(cx).mutation_revision(&item.detail_key()),
                Some(own)
            );
            // The own revision is never polled: another person's update arrives first.
            item.updated_at = "2026-10-22T12:00:00Z".into();
            let generation = inbox.stage_fixture_poll(cx);
            inbox.finish_fixture_poll(
                generation,
                Snapshot {
                    items: vec![item.clone()],
                    ..Default::default()
                },
                cx,
            );
            assert!(inbox.state.seen.unseen(&item));
            assert!(
                inbox
                    .feed
                    .read(cx)
                    .mutation_revision(&item.detail_key())
                    .is_none()
            );
            assert_eq!(
                inbox
                    .feed
                    .read(cx)
                    .mutation_revision(&previous.detail_key()),
                Some(own),
                "a revision from a replacement server cannot consume another server's receipt"
            );
            subscription
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(*changes.borrow(), vec![0, 1]);
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
            Rc::make_mut(&mut inbox.snapshot).items = vec![item.clone()];
            inbox.selected = Some(item.detail_key());
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

#[gpui::test]
fn an_unconnected_source_spans_the_page_without_a_detail_pane(cx: &mut TestAppContext) {
    let window = inbox_window(cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(
        window
            .update(visual, |inbox, _, _| inbox.unavailable())
            .unwrap()
    );
    let list = visual.debug_bounds("connector-list").unwrap();
    assert!(
        f32::from(list.size.width) > 1000.,
        "the explanation uses the whole page, not the list column: {list:?}"
    );
}
