use super::*;
use gpui::{Bounds, TestAppContext, WindowBounds, WindowOptions, point, size};

struct BoardTile {
    board: Entity<TodoBoardView>,
    width: f32,
}

impl Render for BoardTile {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(div().w(px(self.width)).h_full().child(self.board.clone()))
    }
}

fn tile_window(
    cx: &mut TestAppContext,
    width: f32,
    columns: bool,
    height: f32,
) -> gpui::WindowHandle<BoardTile> {
    cx.update(|cx| {
        crate::input::bind(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1200.), px(height)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| BoardTile {
                    board: cx.new(|cx| {
                        let store: Database = std::sync::Arc::new(std::sync::Mutex::new(
                            vorkspace_core::store::Store::open(std::path::Path::new(":memory:"))
                                .unwrap(),
                        ));
                        let mut view = TodoBoardView::new(store, columns, cx);
                        view.set_board(Some(TodoBoard::Personal), cx);
                        view
                    }),
                    width,
                })
            },
        )
        .unwrap()
    })
}

#[gpui::test]
fn columns_wrap_at_tile_width_and_stay_four_across_when_space_fits(cx: &mut TestAppContext) {
    let narrow_tile = tile_window(cx, 420., true, 800.);
    {
        let visual = &mut gpui::VisualTestContext::from_window(narrow_tile.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        let todo = visual.debug_bounds("todo-column-todo").unwrap();
        let progress = visual.debug_bounds("todo-column-inProgress").unwrap();
        let review = visual.debug_bounds("todo-column-inReview").unwrap();
        let done = visual.debug_bounds("todo-column-done").unwrap();
        assert_eq!(todo.origin.y, progress.origin.y);
        assert_eq!(review.origin.y, done.origin.y);
        assert!(review.origin.y > todo.origin.y);
        assert!(todo.size.width >= px(180.));
        assert!(progress.size.width >= px(180.));
    }
    let wide_tile = tile_window(cx, 1000., true, 800.);
    {
        let visual = &mut gpui::VisualTestContext::from_window(wide_tile.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        let y = visual.debug_bounds("todo-column-todo").unwrap().origin.y;
        for selector in [
            "todo-column-inProgress",
            "todo-column-inReview",
            "todo-column-done",
        ] {
            assert_eq!(visual.debug_bounds(selector).unwrap().origin.y, y);
        }
    }
    let sidebar = tile_window(cx, 420., false, 800.);
    let visual = &mut gpui::VisualTestContext::from_window(sidebar.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let todo = visual.debug_bounds("todo-column-todo").unwrap();
    let progress = visual.debug_bounds("todo-column-inProgress").unwrap();
    let review = visual.debug_bounds("todo-column-inReview").unwrap();
    let done = visual.debug_bounds("todo-column-done").unwrap();
    assert!(todo.origin.y < progress.origin.y);
    assert!(progress.origin.y < review.origin.y);
    assert!(review.origin.y < done.origin.y);
}

#[gpui::test]
fn floating_create_keeps_its_draft_and_submits_from_the_popover(cx: &mut TestAppContext) {
    let tile = tile_window(cx, 420., true, 600.);
    let visual = &mut gpui::VisualTestContext::from_window(tile.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let board = visual.debug_bounds("todo-board").unwrap();
    let create = visual.debug_bounds("todo-create").unwrap();
    assert_eq!(create.right(), board.right() - px(16.));
    assert_eq!(create.bottom(), board.bottom() - px(16.));
    assert!(visual.debug_bounds("todo-create-form").is_none());

    visual.simulate_click(create.center(), Default::default());
    assert!(visual.debug_bounds("todo-create-form").is_some());
    visual.simulate_input("Keep this task draft");
    visual.simulate_keystrokes("escape");
    assert!(visual.debug_bounds("todo-create-form").is_none());
    tile.update(visual, |tile, window, cx| {
        let board = tile.board.read(cx);
        assert!(board.create_focus.is_focused(window));
        assert_eq!(
            board.draft.read(cx).content.as_ref(),
            "Keep this task draft"
        );
        assert!(board.items.is_empty());
    })
    .unwrap();

    visual.simulate_click(create.center(), Default::default());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("todo-create-form").is_none());
    tile.update(visual, |tile, window, cx| {
        let board = tile.board.read(cx);
        assert!(board.create_focus.is_focused(window));
        assert!(board.draft.read(cx).content.is_empty());
        let stored = locked(&board.store)
            .unwrap()
            .list_todos(&TodoBoard::Personal)
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].title, "Keep this task draft");
        assert_eq!(board.items, stored);
    })
    .unwrap();
}

#[gpui::test]
fn task_details_overlay_preserves_the_board_and_keeps_edit_actions_visible(
    cx: &mut TestAppContext,
) {
    let tile = tile_window(cx, 420., true, 600.);
    tile.update(cx, |tile, _, cx| {
        let store = tile.board.read(cx).store.clone();
        let board = TodoBoard::Personal;
        let prerequisite = locked(&store)
            .unwrap()
            .add_todo(&board, "Review checklist", TodoStatus::Todo)
            .unwrap();
        let task = locked(&store)
            .unwrap()
            .add_todo_with_details(
                &board,
                "Confirm signed desktop build preserves saved projects and conversations",
                TodoStatus::Todo,
                "Check the in-place upgrade and record the outcome.",
                &["https://example.com/release-plan.png".into()],
                &[prerequisite.id],
            )
            .unwrap();
        let items = locked(&store).unwrap().list_todos(&board).unwrap();
        tile.board.update(cx, |view, cx| {
            view.generation += 1;
            view.items = items;
            view.open_details(&task, cx);
            assert_eq!(
                view.details
                    .as_ref()
                    .unwrap()
                    .title
                    .read(cx)
                    .selection_utf16(),
                0..0
            );
        });
    })
    .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(tile.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("todo-board-columns").is_some());
    assert!(visual.debug_bounds("todo-details-dialog").is_some());
    assert!(visual.debug_bounds("todo-details-save").is_none());
    let edit = visual.debug_bounds("todo-details-edit").unwrap();
    visual.simulate_click(edit.center(), Default::default());
    let detail = visual.debug_bounds("todo-details").unwrap();
    let scroller = visual.debug_bounds("todo-details-scroll").unwrap();
    let footer = visual.debug_bounds("todo-details-footer").unwrap();
    let save = visual.debug_bounds("todo-details-save").unwrap();
    let files = visual.debug_bounds("todo-add-files").unwrap();
    let link = visual.debug_bounds("todo-add-url").unwrap();
    assert!(visual.debug_bounds("todo-board-columns").is_some());
    assert!(visual.debug_bounds("todo-details-close").is_some());
    assert!(detail.size.width <= px(760.));
    assert!(detail.origin.x > px(0.));
    assert!(detail.size.height <= px(600.));
    assert!(scroller.origin.y < footer.origin.y);
    assert!(save.origin.y >= footer.origin.y);
    assert!(save.origin.y + save.size.height <= footer.origin.y + footer.size.height);
    assert!(files.origin.y >= scroller.origin.y);
    assert!(files.origin.y + files.size.height <= scroller.origin.y + scroller.size.height);
    assert!(link.origin.y >= scroller.origin.y);
    assert!(link.origin.y + link.size.height <= scroller.origin.y + scroller.size.height);
    assert!(files.size.width >= px(72.));
    assert!(link.size.width >= px(60.));
}

fn board_window(cx: &mut TestAppContext) -> (gpui::WindowHandle<TodoBoardView>, Database) {
    let store: Database = std::sync::Arc::new(std::sync::Mutex::new(
        vorkspace_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
    ));
    let window = cx.update(|cx| {
        crate::input::bind(cx);
        let store = store.clone();
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = TodoBoardView::new(store, true, cx);
                view.set_board(Some(TodoBoard::Personal), cx);
                view
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    (window, store)
}

#[gpui::test]
fn adding_advancing_and_deleting_reach_the_store_and_every_view(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    let other = {
        let store = store.clone();
        cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = TodoBoardView::new(store, false, cx);
                    view.set_board(Some(TodoBoard::Personal), cx);
                    view
                })
            })
            .unwrap()
        })
    };
    window
        .update(cx, |view, _, cx| {
            view.draft
                .update(cx, |draft, cx| draft.set_text("Ship boards".into(), cx));
            view.add(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let item = locked(&store)
        .unwrap()
        .list_todos(&TodoBoard::Personal)
        .unwrap()[0]
        .clone();
    assert_eq!(item.title, "Ship boards");
    // The second view of the same board reloads too.
    other
        .update(cx, |view, _, _| {
            assert_eq!(view.items(), std::slice::from_ref(&item))
        })
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            view.advance(item.id.clone(), item.status, cx)
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        locked(&store)
            .unwrap()
            .list_todos(&TodoBoard::Personal)
            .unwrap()[0]
            .status,
        TodoStatus::InProgress
    );
    window
        .update(cx, |view, _, cx| view.delete(item.id.clone(), cx))
        .unwrap();
    cx.run_until_parked();
    assert!(
        locked(&store)
            .unwrap()
            .list_todos(&TodoBoard::Personal)
            .unwrap()
            .is_empty()
    );
    other
        .update(cx, |view, _, _| assert!(view.items().is_empty()))
        .unwrap();
}

#[gpui::test]
fn a_refused_title_stays_in_the_field_with_its_reason(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    let long = "x".repeat(vorkspace_core::todos::MAX_TITLE + 1);
    window
        .update(cx, |view, _, cx| {
            view.draft
                .update(cx, |draft, cx| draft.set_text(long.clone(), cx));
            view.add(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        locked(&store)
            .unwrap()
            .list_todos(&TodoBoard::Personal)
            .unwrap()
            .is_empty()
    );
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.draft.read(cx).content.to_string(), long);
            assert!(view.change_error.as_deref().unwrap().contains("limited"));
        })
        .unwrap();
    // An unrelated reload keeps the reason visible.
    cx.update(changed);
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert!(view.change_error.is_some()))
        .unwrap();
}

#[gpui::test]
fn changes_reach_the_store_in_the_order_they_were_made(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    let item = locked(&store)
        .unwrap()
        .add_todo(&TodoBoard::Personal, "Twice", TodoStatus::Todo)
        .unwrap();
    cx.update(changed);
    cx.run_until_parked();
    // Three clicks before any write lands.
    window
        .update(cx, |view, _, cx| {
            view.advance(item.id.clone(), TodoStatus::Todo, cx);
            view.advance(item.id.clone(), TodoStatus::InProgress, cx);
            view.advance(item.id.clone(), TodoStatus::InReview, cx);
            // A reload between the writes would show the first one.
            view.refresh(cx);
            assert_eq!(view.items()[0].status, TodoStatus::Done);
        })
        .unwrap();
    cx.run_until_parked();
    let stored = locked(&store)
        .unwrap()
        .get_todo(&TodoBoard::Personal, &item.id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, TodoStatus::Done);
    window
        .update(cx, |view, _, _| {
            assert_eq!(view.pending_changes, 0);
            assert_eq!(view.items()[0].status, TodoStatus::Done);
        })
        .unwrap();
}

#[gpui::test]
fn local_drops_use_local_ordering_anchors_on_mixed_issue_boards(cx: &mut TestAppContext) {
    for (status, remote_target, local_after) in [
        (TodoStatus::Todo, true, true),
        (TodoStatus::InProgress, true, true),
        (TodoStatus::InProgress, true, false),
        (TodoStatus::Todo, false, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let board = TodoBoard::Project(root.path().to_string_lossy().into_owned());
        let (window, store) = board_window(cx);
        let task = locked(&store)
            .unwrap()
            .add_todo(&board, "Move me", TodoStatus::Todo)
            .unwrap();
        let anchor = locked(&store)
            .unwrap()
            .add_todo(&board, "Local anchor", status)
            .unwrap();
        window
            .update(cx, |view, _, cx| view.set_board(Some(board.clone()), cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                let mut issue = anchor.clone();
                issue.id = "github:test/project#7".into();
                issue.issue = Some(vorkspace_core::todos::TodoIssue {
                    repo: "test/project".into(),
                    number: 7,
                    url: "https://github.com/test/project/issues/7".into(),
                    labels: vec![],
                    dependencies_truncated: false,
                });
                let target = if remote_target {
                    issue.id.clone()
                } else {
                    anchor.id.clone()
                };
                view.items = if local_after {
                    vec![task.clone(), issue, anchor.clone()]
                } else {
                    vec![anchor.clone(), issue, task.clone()]
                };
                view.move_to(
                    &DraggedTodo {
                        board: board.key(),
                        id: task.id.clone(),
                        title: task.title.clone().into(),
                    },
                    status,
                    Some(target),
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        let stored = locked(&store)
            .unwrap()
            .get_todo(&board, &task.id)
            .unwrap()
            .unwrap();
        let anchor = locked(&store)
            .unwrap()
            .get_todo(&board, &anchor.id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, status);
        if local_after {
            assert!(
                stored.position < anchor.position,
                "the next local card is the ordering anchor"
            );
        } else {
            assert!(
                stored.position > anchor.position,
                "a remote card without a following local card appends"
            );
        }
        window
            .update(cx, |view, _, _| assert!(view.change_error.is_none()))
            .unwrap();
    }
}

#[gpui::test]
fn a_drop_from_another_board_is_ignored(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    let elsewhere = locked(&store)
        .unwrap()
        .add_todo(&TodoBoard::Bot("b".into()), "theirs", TodoStatus::Todo)
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            let dragged = DraggedTodo {
                board: TodoBoard::Bot("b".into()).key(),
                id: elsewhere.id.clone(),
                title: "theirs".into(),
            };
            view.move_to(&dragged, TodoStatus::Done, None, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let theirs = locked(&store)
        .unwrap()
        .get_todo(&TodoBoard::Bot("b".into()), &elsewhere.id)
        .unwrap()
        .unwrap();
    assert_eq!(theirs.status, TodoStatus::Todo);
}

#[gpui::test]
fn details_editor_saves_description_attachments_and_dependencies(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    let board = TodoBoard::Personal;
    let dependency = locked(&store)
        .unwrap()
        .add_todo(&board, "Prerequisite", TodoStatus::Todo)
        .unwrap();
    let task = locked(&store)
        .unwrap()
        .add_todo(&board, "Ship", TodoStatus::Todo)
        .unwrap();
    cx.update(changed);
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.open_details(&task, cx);
            let editor = view.details.as_ref().unwrap().clone();
            editor.description.update(cx, |input, cx| {
                input.set_text("Explain the work".into(), cx)
            });
            editor.url.update(cx, |input, cx| {
                input.set_text("https://example.com/design".into(), cx)
            });
            view.toggle_dependency(dependency.id.clone(), cx);
            assert_eq!(view.dependency_label(&task, &dependency.id), "Prerequisite");
            view.save_details(cx);
            assert_eq!(view.pending_changes, 0);
            assert!(
                view.change_error
                    .as_deref()
                    .unwrap()
                    .contains("unfinished attachment link")
            );
            view.stash_details();
            view.open_details(&task, cx);
            let reopened = view.details.as_ref().unwrap();
            assert_eq!(
                reopened.url.read(cx).content.as_ref(),
                "https://example.com/design"
            );
            assert_eq!(
                reopened.description.read(cx).content.as_ref(),
                "Explain the work"
            );
            view.add_attachment_url(cx);
            view.save_details(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let saved = locked(&store)
        .unwrap()
        .get_todo(&board, &task.id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.description, "Explain the work");
    assert_eq!(saved.attachments, ["https://example.com/design"]);
    assert_eq!(saved.dependencies, [dependency.id]);
}

#[gpui::test]
fn github_detail_save_does_not_queue_an_unfinished_attachment_link(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    let mut issue = locked(&store)
        .unwrap()
        .add_todo(&TodoBoard::Personal, "Issue", TodoStatus::Todo)
        .unwrap();
    issue.id = "github:example/project#42".into();
    issue.issue = Some(vorkspace_core::todos::TodoIssue {
        repo: "example/project".into(),
        number: 42,
        url: "https://github.com/example/project/issues/42".into(),
        labels: Vec::new(),
        dependencies_truncated: false,
    });
    issue.sync = TodoSync::Synced;
    let project = tempfile::tempdir().unwrap();
    let board = TodoBoard::Project(project.path().to_string_lossy().into_owned());
    window
        .update(cx, |view, window, cx| {
            view.board = Some(board.clone());
            view.items = vec![issue.clone()];
            view.present_details(&issue, window, cx);
            view.detail_editing = true;
            cx.notify();
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    for value in ["https://example.com/design", "not a complete URL"] {
        window
            .update(visual, |view, _, cx| {
                view.details
                    .as_ref()
                    .unwrap()
                    .url
                    .update(cx, |input, cx| input.set_text(value.into(), cx));
            })
            .unwrap();
        let save = visual.debug_bounds("todo-details-save").unwrap();
        visual.simulate_click(save.center(), Default::default());
        window
            .update(visual, |view, _, cx| {
                assert_eq!(view.pending_changes, 0);
                assert!(
                    view.change_error
                        .as_deref()
                        .unwrap()
                        .contains("unfinished attachment link")
                );
                assert_eq!(
                    view.details.as_ref().unwrap().url.read(cx).content.as_ref(),
                    value
                );
            })
            .unwrap();
    }
    assert!(
        locked(&store)
            .unwrap()
            .pending_remote_todo_edits(&board)
            .unwrap()
            .is_empty()
    );
}

#[gpui::test]
fn unfinished_detail_draft_survives_close_other_task_and_board_switch(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    let first = locked(&store)
        .unwrap()
        .add_todo(&TodoBoard::Personal, "First", TodoStatus::Todo)
        .unwrap();
    let second = locked(&store)
        .unwrap()
        .add_todo(&TodoBoard::Personal, "Second", TodoStatus::Todo)
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            view.open_details(&first, cx);
            view.details
                .as_ref()
                .unwrap()
                .description
                .update(cx, |input, cx| {
                    input.set_text("Unfinished draft".into(), cx)
                });
            view.open_details(&second, cx);
            view.open_details(&first, cx);
            assert_eq!(
                view.details
                    .as_ref()
                    .unwrap()
                    .description
                    .read(cx)
                    .content
                    .to_string(),
                "Unfinished draft"
            );
            view.stash_details();
            view.open_details(&first, cx);
            assert_eq!(
                view.details
                    .as_ref()
                    .unwrap()
                    .description
                    .read(cx)
                    .content
                    .to_string(),
                "Unfinished draft"
            );
            view.set_board(
                Some(TodoBoard::Project("/missing/todo-draft-test".into())),
                cx,
            );
            view.set_board(Some(TodoBoard::Personal), cx);
            view.open_details(&first, cx);
            assert_eq!(
                view.details
                    .as_ref()
                    .unwrap()
                    .description
                    .read(cx)
                    .content
                    .to_string(),
                "Unfinished draft"
            );
        })
        .unwrap();
}

#[gpui::test]
fn rejected_detail_save_keeps_the_draft_and_error(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    let task = locked(&store)
        .unwrap()
        .add_todo(&TodoBoard::Personal, "Ship", TodoStatus::Todo)
        .unwrap();
    cx.update(changed);
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.open_details(&task, cx);
            view.details
                .as_ref()
                .unwrap()
                .description
                .update(cx, |input, cx| input.set_text("Keep this draft".into(), cx));
            view.toggle_dependency("missing-task".into(), cx);
            view.save_details(cx);
            view.set_board(
                Some(TodoBoard::Project("/missing/todo-error-test".into())),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.set_board(Some(TodoBoard::Personal), cx);
            view.open_details(&task, cx);
            assert!(
                view.change_error
                    .as_deref()
                    .unwrap()
                    .contains("not on this board")
            );
            assert_eq!(
                view.details
                    .as_ref()
                    .unwrap()
                    .description
                    .read(cx)
                    .content
                    .to_string(),
                "Keep this draft"
            );
            assert_eq!(view.pending_changes, 0);
        })
        .unwrap();
}

#[gpui::test]
fn publishing_requires_saving_the_open_or_reopened_detail_draft(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    // Simulate a linked board in the view while keeping the fixture entirely local.
    let project = tempfile::tempdir().unwrap();
    let board = TodoBoard::Project(project.path().to_string_lossy().into_owned());
    let (task, dependency) = {
        let store = locked(&store).unwrap();
        (
            store
                .add_todo(&board, "Original", TodoStatus::Todo)
                .unwrap(),
            store
                .add_todo(&board, "Dependency", TodoStatus::Todo)
                .unwrap(),
        )
    };
    window
        .update(cx, |view, _, cx| view.set_board(Some(board.clone()), cx))
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.github_repo = Some("example/project".into());
            cx.notify();
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    let click = |selector, visual: &mut gpui::VisualTestContext| {
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Default::default());
    };
    let card: &'static str = Box::leak(format!("todo-{}", task.id).into_boxed_str());
    click(card, visual);
    click("todo-details-edit", visual);
    window
        .update(visual, |view, _, cx| {
            let editor = view.details.as_ref().unwrap();
            editor.title.update(cx, |input, cx| {
                input.set_text("Unfinished title".into(), cx)
            });
            editor.description.update(cx, |input, cx| {
                input.set_text("Unfinished description".into(), cx)
            });
            view.append_attachment("https://example.com/design".into(), cx);
            view.toggle_dependency(dependency.id.clone(), cx);
        })
        .unwrap();
    for reopen in [false, true] {
        if reopen {
            click("todo-details-close", visual);
            click(card, visual);
            click("todo-details-edit", visual);
        }
        click("todo-publish", visual);
        window
            .update(visual, |view, _, cx| {
                assert_eq!(view.pending_changes, 0);
                assert!(
                    view.change_error
                        .as_deref()
                        .unwrap()
                        .contains("Save your changes before publishing")
                );
                let editor = view.details.as_ref().unwrap();
                assert_eq!(editor.title.read(cx).content.as_ref(), "Unfinished title");
                assert_eq!(
                    editor.description.read(cx).content.as_ref(),
                    "Unfinished description"
                );
                assert_eq!(
                    TodoBoardView::reference_lines(&editor.attachments, cx),
                    ["https://example.com/design"]
                );
                assert_eq!(
                    TodoBoardView::dependency_values(&editor.dependencies, cx).as_slice(),
                    std::slice::from_ref(&dependency.id)
                );
            })
            .unwrap();
    }
    let store = locked(&store).unwrap();
    assert_eq!(store.get_todo(&board, &task.id).unwrap(), Some(task));
    assert!(store.pending_todo_submissions(&board).unwrap().is_empty());
}

#[gpui::test]
fn stale_local_detail_save_preserves_both_edits_until_explicit_reload(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    let board = TodoBoard::Personal;
    let task = locked(&store)
        .unwrap()
        .add_todo_with_details(
            &board,
            "Original",
            TodoStatus::Todo,
            "Old description",
            &[],
            &[],
        )
        .unwrap();
    cx.update(changed);
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.open_details(&task, cx);
            view.detail_editing = true;
            view.details
                .as_ref()
                .unwrap()
                .title
                .update(cx, |input, cx| input.set_text("My new title".into(), cx));
        })
        .unwrap();
    // Leave the editor's view stale so the persistence boundary must catch the race.
    let updated = locked(&store)
        .unwrap()
        .todo_action(
            &board,
            "todos.update",
            &serde_json::json!({"id":task.id,"description":"Newer description"}),
        )
        .unwrap();
    let revision = updated["updatedAt"].as_i64().unwrap();
    assert!(revision > task.updated_at);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.items[0].updated_at, task.updated_at);
            view.save_details(cx);
            assert_eq!(view.pending_changes, 1);
        })
        .unwrap();
    cx.run_until_parked();
    let saved = locked(&store)
        .unwrap()
        .get_todo(&board, &task.id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.title, "Original");
    assert_eq!(saved.description, "Newer description");
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| {
            assert!(
                view.change_error
                    .as_deref()
                    .unwrap()
                    .contains("draft is preserved")
            );
            let editor = view.details.as_ref().unwrap();
            assert_eq!(editor.revision, task.updated_at);
            assert_eq!(editor.title.read(cx).content.as_ref(), "My new title");
        })
        .unwrap();
    let reload = visual.debug_bounds("todo-reload-details").unwrap();
    visual.simulate_click(reload.center(), Default::default());
    window
        .update(visual, |view, _, cx| {
            let editor = view.details.as_ref().unwrap();
            assert_eq!(editor.revision, revision);
            assert_eq!(
                editor.description.read(cx).content.as_ref(),
                "Newer description"
            );
            editor
                .title
                .update(cx, |input, cx| input.set_text("My new title".into(), cx));
            view.save_details(cx);
        })
        .unwrap();
    visual.run_until_parked();
    let saved = locked(&store)
        .unwrap()
        .get_todo(&board, &task.id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.title, "My new title");
    assert_eq!(saved.description, "Newer description");
}

#[gpui::test]
fn refreshed_issue_does_not_replace_the_editors_original_revision(cx: &mut TestAppContext) {
    let (window, store) = board_window(cx);
    let mut issue = locked(&store)
        .unwrap()
        .add_todo(&TodoBoard::Personal, "Original", TodoStatus::Todo)
        .unwrap();
    issue.issue = Some(vorkspace_core::todos::TodoIssue {
        repo: "example/project".into(),
        number: 42,
        url: "https://github.com/example/project/issues/42".into(),
        labels: vec![],
        dependencies_truncated: false,
    });
    issue.sync = vorkspace_core::todos::TodoSync::Synced;
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.items = vec![issue.clone()];
            view.open_details(&issue, cx);
            view.details
                .as_ref()
                .unwrap()
                .description
                .update(cx, |input, cx| {
                    input.set_text("My unfinished edit".into(), cx)
                });
            let mut refreshed = issue.clone();
            refreshed.updated_at += 1;
            refreshed.description = "Someone else's newer edit".into();
            refreshed.issue.as_mut().unwrap().dependencies_truncated = true;
            view.items = vec![refreshed.clone()];
            view.stash_details();
            view.open_details(&refreshed, cx);
            assert_eq!(view.details.as_ref().unwrap().revision, issue.updated_at);
            assert!(view.details.as_ref().unwrap().dependencies_complete);
            view.detail_editing = true;
            view.save_details(cx);
            assert_eq!(view.pending_changes, 0);
            assert!(
                view.change_error
                    .as_deref()
                    .unwrap()
                    .contains("draft is preserved")
            );
            assert_eq!(
                view.details
                    .as_ref()
                    .unwrap()
                    .description
                    .read(cx)
                    .content
                    .as_ref(),
                "My unfinished edit"
            );
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    let reload = visual.debug_bounds("todo-reload-details").unwrap();
    visual.simulate_click(reload.center(), Default::default());
    window
        .update(visual, |view, _, cx| {
            let editor = view.details.as_ref().unwrap();
            assert_eq!(editor.revision, issue.updated_at + 1);
            assert!(!editor.dependencies_complete);
            assert_eq!(
                editor.description.read(cx).content.as_ref(),
                "Someone else's newer edit"
            );
            assert!(view.change_error.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn external_publication_preserves_open_and_stashed_task_drafts(cx: &mut TestAppContext) {
    for (stashed, linked_draft) in [(false, true), (true, false), (true, true)] {
        let (window, store) = board_window(cx);
        let directory = tempfile::tempdir().unwrap();
        let board = TodoBoard::Project(directory.path().to_string_lossy().into_owned());
        let task = locked(&store)
            .unwrap()
            .add_todo(&board, "Stored task", TodoStatus::Todo)
            .unwrap();
        let mut issue = task.clone();
        issue.id = "github:example/Project#12".into();
        issue.title = "Published task".into();
        issue.issue = Some(vorkspace_core::todos::TodoIssue {
            repo: "example/Project".into(),
            number: 12,
            url: "https://github.com/example/Project/issues/12".into(),
            labels: vec![],
            dependencies_truncated: false,
        });
        issue.sync = TodoSync::Synced;
        // Independent revision clocks can have the same numeric value.
        assert_eq!(task.updated_at, issue.updated_at);

        window
            .update(cx, |view, _, cx| view.set_board(Some(board.clone()), cx))
            .unwrap();
        cx.run_until_parked();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        let draft = [
            "My draft title",
            "My draft description",
            "notes/design.md\nhttps://example.com/reference",
            "dependency-id, #23",
            "https://example.com/unfinished",
        ];
        let assert_draft = |view: &TodoBoardView, cx: &App| {
            let editor = view.details.as_ref().unwrap();
            assert_eq!(editor.id, task.id);
            assert_eq!(editor.revision, task.updated_at);
            for (field, text) in [
                &editor.title,
                &editor.description,
                &editor.attachments,
                &editor.dependencies,
                &editor.url,
            ]
            .into_iter()
            .zip(draft)
            {
                assert_eq!(field.read(cx).content.as_ref(), text);
            }
        };
        window
            .update(visual, |view, window, cx| {
                if linked_draft {
                    // An earlier list can expose the issue before it sees
                    // the publication receipt. Retain that distinct draft too.
                    view.install_snapshot(service::BoardSnapshot::project(
                        vec![task.clone()],
                        vec![],
                        Some(vec![issue.clone()]),
                    ));
                    view.open_details(&issue, cx);
                    view.details
                        .as_ref()
                        .unwrap()
                        .title
                        .update(cx, |input, cx| {
                            input.set_text("Earlier GitHub draft".into(), cx)
                        });
                }
                view.present_details(&task, window, cx);
                view.detail_editing = true;
                let editor = view.details.as_ref().unwrap();
                for (field, text) in [
                    &editor.title,
                    &editor.description,
                    &editor.attachments,
                    &editor.dependencies,
                    &editor.url,
                ]
                .into_iter()
                .zip(draft)
                {
                    field.update(cx, |input, cx| input.set_text(text.into(), cx));
                }
                if stashed {
                    view.dismiss_details(window, cx);
                }
            })
            .unwrap();

        let snapshot = {
            let store = locked(&store).unwrap();
            store
                .begin_todo_publish(&board, &task.id, "example/project", task.updated_at)
                .unwrap();
            store.finish_todo_publish(&board, &task.id, &issue).unwrap();
            service::BoardSnapshot::project(
                store.list_todos(&board).unwrap(),
                vec![],
                Some(vec![issue.clone()]),
            )
        };
        assert_eq!(snapshot.items, vec![issue.clone()]);
        let stale_save = service::action(
            &store,
            &board,
            "todos.update",
            &serde_json::json!({"id":task.id,"title":"Stale edit","expectedUpdatedAt":task.updated_at}),
        );
        assert_eq!(stale_save.unwrap_err(), STALE_DETAILS);

        window
            .update(visual, |view, window, cx| {
                view.install_snapshot(snapshot);
                if stashed {
                    view.present_details(&issue, window, cx);
                }
                if !(stashed && linked_draft) {
                    view.detail_editing = true;
                }
                assert_eq!(view.open_detail_item().unwrap().id, issue.id);
                cx.notify();
            })
            .unwrap();
        if stashed && linked_draft {
            let assert_linked_draft = |view: &TodoBoardView, cx: &App| {
                let editor = view.details.as_ref().unwrap();
                assert_eq!(editor.id, issue.id);
                assert!(editor.matches_revision(view.open_detail_item().unwrap()));
                assert_eq!(
                    editor.title.read(cx).content.as_ref(),
                    "Earlier GitHub draft"
                );
            };
            window
                .update(visual, |view, _, cx| assert_linked_draft(view, cx))
                .unwrap();
            assert!(visual.debug_bounds("todo-details-edit").is_some());
            assert!(visual.debug_bounds("todo-reload-details").is_none());
            let switch = |visual: &mut gpui::VisualTestContext| {
                let bounds = visual.debug_bounds("todo-switch-draft").unwrap();
                visual.simulate_click(bounds.center(), Default::default());
            };
            switch(visual);
            window
                .update(visual, |view, _, cx| {
                    assert!(view.detail_editing);
                    assert_draft(view, cx);
                })
                .unwrap();
            switch(visual);
            window
                .update(visual, |view, _, cx| assert_linked_draft(view, cx))
                .unwrap();
            assert!(visual.debug_bounds("todo-reload-details").is_none());
            switch(visual);
        }

        window
            .update(visual, |view, _, cx| {
                assert_draft(view, cx);
                view.save_details(cx);
                assert_eq!(view.pending_changes, 0);
                assert_eq!(view.change_error.as_deref(), Some(STALE_DETAILS));
                if linked_draft {
                    let key = view.details_key(&issue.id).unwrap();
                    assert_eq!(
                        view.saved_details[&key].title.read(cx).content.as_ref(),
                        "Earlier GitHub draft"
                    );
                }
                cx.notify();
            })
            .unwrap();
        assert!(visual.debug_bounds("todo-details-dialog").is_some());
        assert!(visual.debug_bounds("todo-reload-details").is_some());

        // A pending GitHub edit can supply the displayed issue even offline.
        let attempt = locked(&store)
            .unwrap()
            .begin_remote_todo_edit(&board, &issue, &serde_json::json!({"status":"done"}))
            .unwrap();
        visual.cx.update(changed);
        visual.run_until_parked();
        window
            .update(visual, |view, _, cx| {
                let item = view.open_detail_item().unwrap();
                assert_eq!(item.id, issue.id);
                assert!(item.pending_edit.is_some());
                assert_draft(view, cx);
            })
            .unwrap();
        locked(&store)
            .unwrap()
            .clear_remote_todo_edit(&board, &issue.id, &attempt)
            .unwrap();

        // A refresh without a GitHub origin displays the durable local receipt.
        visual.cx.update(changed);
        visual.run_until_parked();
        window
            .update(visual, |view, window, cx| {
                assert_eq!(view.open_detail_item().unwrap().id, task.id);
                assert_draft(view, cx);
                assert!(
                    !view
                        .details
                        .as_ref()
                        .unwrap()
                        .matches_revision(&view.items[0])
                );
                window.remove_window();
            })
            .unwrap();
        let store = locked(&store).unwrap();
        assert_eq!(
            store.get_todo(&board, &task.id).unwrap().unwrap().title,
            task.title
        );
        assert!(store.pending_remote_todo_edits(&board).unwrap().is_empty());
    }
}

#[gpui::test]
fn cards_open_details_for_local_and_github_items_and_escape_returns_to_the_board(
    cx: &mut TestAppContext,
) {
    let (window, store) = board_window(cx);
    let task = locked(&store)
        .unwrap()
        .add_todo_with_details(
            &TodoBoard::Personal,
            "Read the entire issue",
            TodoStatus::Todo,
            &"A detailed description that continues beyond the small card preview.\n".repeat(30),
            &[],
            &[],
        )
        .unwrap();
    cx.update(changed);
    cx.run_until_parked();
    let selector: &'static str = Box::leak(format!("todo-{}", task.id).into_boxed_str());
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    for linked in [false, true] {
        window
            .update(visual, |view, _, cx| {
                if linked {
                    view.items[0].issue = Some(vorkspace_core::todos::TodoIssue {
                        repo: "example/project".into(),
                        number: 42,
                        url: "https://github.com/example/project/issues/42".into(),
                        labels: Vec::new(),
                        dependencies_truncated: false,
                    });
                }
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        let card = visual.debug_bounds(selector).unwrap();
        let preview = visual.debug_bounds("todo-description-preview").unwrap();
        assert!(preview.size.height <= px(60.));
        visual.simulate_click(card.center(), Default::default());
        assert!(visual.debug_bounds("todo-details-dialog").is_some());
        assert!(visual.debug_bounds("todo-board-columns").is_some());
        window
            .update(visual, |view, _, cx| {
                assert_eq!(
                    view.details
                        .as_ref()
                        .unwrap()
                        .description
                        .read(cx)
                        .content
                        .as_ref(),
                    task.description
                );
                assert!(!view.detail_editing);
            })
            .unwrap();
        visual.simulate_keystrokes("escape");
        assert!(visual.debug_bounds("todo-details-dialog").is_none());
        assert!(visual.debug_bounds(selector).is_some());
    }
}
