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
            view.append_attachment("https://example.com/design".into(), cx);
            view.toggle_dependency(dependency.id.clone(), cx);
            assert_eq!(view.dependency_label(&dependency.id), "Prerequisite");
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
