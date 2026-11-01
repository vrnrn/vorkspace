use super::*;
use gpui::TestAppContext;

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
    // Two clicks before either write lands.
    window
        .update(cx, |view, _, cx| {
            view.advance(item.id.clone(), TodoStatus::Todo, cx);
            view.advance(item.id.clone(), TodoStatus::InProgress, cx);
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
