//! A very simple to-do board: To do, In progress and Done. The same view
//! serves a project's sidebar, a Bot's details and the To-dos page; narrow
//! hosts stack the columns, the page lays them side by side.
use crate::{
    input::{self, Input},
    popover::{Anchor, AnchorSlot, popover},
    theme::*,
};
use gpui::{
    App, Context, CursorStyle, Entity, FocusHandle, Focusable, FontWeight, Global, MouseButton,
    Role, SharedString, Subscription, Window, div, prelude::*, px,
};
use vorkspace_core::todos::{TodoBoard, TodoItem, TodoStatus};
use vorkspace_runtime::runtime::{Database, locked};

/// Bumped after any board changes, so every open view of it reloads.
#[derive(Default)]
pub(crate) struct TodoRevision(pub u64);
impl Global for TodoRevision {}

pub(crate) fn changed(cx: &mut App) {
    let next = cx.try_global::<TodoRevision>().map_or(0, |r| r.0) + 1;
    cx.set_global(TodoRevision(next));
}

type ChangeWork =
    Box<dyn FnOnce(&vorkspace_core::store::Store, &TodoBoard) -> Result<(), String> + Send>;

/// A store change waiting its turn. A view's changes run one at a time, in
/// the order they were made, so a later click can't be overwritten by an
/// earlier one that reached the database second.
struct Change {
    board: TodoBoard,
    work: ChangeWork,
    /// A submitted draft to put back if the change fails.
    draft: Option<String>,
}

#[derive(Clone)]
struct DraggedTodo {
    board: String,
    id: String,
    title: SharedString,
}
impl Render for DraggedTodo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(240.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(gpui::rgb(0x2a2a2a))
            .border_1()
            .border_color(content(0.1))
            .text_size(px(13.))
            .text_color(content(0.9))
            .truncate()
            .child(self.title.clone())
    }
}

pub(crate) struct TodoBoardView {
    store: Database,
    board: Option<TodoBoard>,
    items: Vec<TodoItem>,
    loaded: bool,
    /// Guards against an older load landing after a newer one.
    generation: u64,
    /// The last load's failure.
    error: Option<String>,
    /// The last change's failure; a reload doesn't clear it.
    change_error: Option<String>,
    changes: async_channel::Sender<Change>,
    /// Changes sent and not yet finished. Loads wait for them to finish, so
    /// a reload between two changes can't show the first undoing the second.
    pending_changes: usize,
    /// Side-by-side columns; otherwise stacked.
    columns: bool,
    draft: Entity<Input>,
    editing: Option<(String, Entity<Input>)>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for TodoBoardView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.draft.focus_handle(cx)
    }
}

impl TodoBoardView {
    pub(crate) fn new(store: Database, columns: bool, cx: &mut Context<Self>) -> Self {
        let draft = cx.new(|cx| Input::field("New to-do", "Add a to-do…", content(0.35), cx));
        let subscriptions = vec![
            cx.subscribe(&draft, |this, _, _: &input::Submitted, cx| this.add(cx)),
            cx.observe_global::<TodoRevision>(|this, cx| this.refresh(cx)),
        ];
        let (changes, queue) = async_channel::unbounded::<Change>();
        // Queued changes still run if the view closes; the queue ends once
        // it is empty and the view is gone.
        let worker_store = store.clone();
        cx.spawn(async move |this, cx| {
            while let Ok(Change { board, work, draft }) = queue.recv().await {
                let store = worker_store.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { work(&*locked(&store)?, &board) })
                    .await;
                let idle = queue.is_empty();
                cx.update(|cx| {
                    let _ = this.update(cx, |this, cx| this.finish_change(result, draft, cx));
                    if idle {
                        changed(cx);
                    }
                });
            }
        })
        .detach();
        Self {
            store,
            board: None,
            items: Vec::new(),
            loaded: false,
            generation: 0,
            error: None,
            change_error: None,
            changes,
            pending_changes: 0,
            columns,
            draft,
            editing: None,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn board(&self) -> Option<&TodoBoard> {
        self.board.as_ref()
    }

    /// Open items, once the board has loaded.
    pub(crate) fn open_count(&self) -> Option<usize> {
        self.loaded.then(|| {
            self.items
                .iter()
                .filter(|item| item.status != TodoStatus::Done)
                .count()
        })
    }

    #[cfg(test)]
    pub(crate) fn items(&self) -> &[TodoItem] {
        &self.items
    }

    pub(crate) fn set_board(&mut self, board: Option<TodoBoard>, cx: &mut Context<Self>) {
        if self.board == board {
            return;
        }
        self.commit_edit(cx);
        self.board = board;
        self.items.clear();
        self.loaded = false;
        self.error = None;
        self.change_error = None;
        self.refresh(cx);
        cx.notify();
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        // The last queued change reloads every view when it finishes.
        if self.pending_changes > 0 {
            return;
        }
        let Some(board) = self.board.clone() else {
            return;
        };
        let generation = self.generation;
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.list_todos(&board) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loaded = true;
                match result {
                    Ok(items) => {
                        this.items = items;
                        this.error = None;
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Queues a store change to run off the main thread after this view's
    /// earlier ones; every view reloads once the queue is empty.
    fn change(
        &mut self,
        work: impl FnOnce(&vorkspace_core::store::Store, &TodoBoard) -> Result<(), String>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        self.queue_change(work, None, cx);
    }

    fn queue_change(
        &mut self,
        work: impl FnOnce(&vorkspace_core::store::Store, &TodoBoard) -> Result<(), String>
        + Send
        + 'static,
        draft: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(board) = self.board.clone() else {
            return;
        };
        let change = Change {
            board,
            work: Box::new(work),
            draft,
        };
        if self.changes.try_send(change).is_err() {
            return;
        }
        self.pending_changes += 1;
        // A load already under way predates this change.
        self.generation += 1;
        self.change_error = None;
        cx.notify();
    }

    fn finish_change(
        &mut self,
        result: Result<(), String>,
        draft: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.pending_changes = self.pending_changes.saturating_sub(1);
        if let Err(error) = result {
            self.change_error = Some(error);
            // Put the submission back unless something newer was typed.
            if let Some(title) = draft
                && self.draft.read(cx).content.is_empty()
            {
                self.draft.update(cx, |input, cx| input.set_text(title, cx));
            }
            cx.notify();
        }
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let title = self.draft.read(cx).content.to_string();
        if title.trim().is_empty() {
            return;
        }
        // Keep a title the store would refuse in the field, with the reason.
        if let Err(error) = vorkspace_core::todos::clean_title(&title) {
            self.change_error = Some(error);
            cx.notify();
            return;
        }
        self.draft
            .update(cx, |draft, cx| draft.set_text(String::new(), cx));
        self.queue_change(
            {
                let title = title.clone();
                move |store, board| store.add_todo(board, &title, TodoStatus::Todo).map(|_| ())
            },
            Some(title),
            cx,
        );
    }

    fn advance(&mut self, id: String, status: TodoStatus, cx: &mut Context<Self>) {
        // Show the new status at once; the reload confirms it.
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.status = status.next();
            cx.notify();
        }
        self.change(
            move |store, board| store.move_todo(board, &id, status.next(), None).map(|_| ()),
            cx,
        );
    }

    fn move_to(
        &mut self,
        dragged: &DraggedTodo,
        status: TodoStatus,
        before: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.board.as_ref().map(TodoBoard::key) != Some(dragged.board.clone())
            || before.as_deref() == Some(dragged.id.as_str())
        {
            return;
        }
        let id = dragged.id.clone();
        self.change(
            move |store, board| {
                store
                    .move_todo(board, &id, status, before.as_deref())
                    .map(|_| ())
            },
            cx,
        );
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        self.items.retain(|item| item.id != id);
        cx.notify();
        self.change(move |store, board| store.delete_todo(board, &id), cx);
    }

    fn begin_edit(&mut self, item: &TodoItem, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(cx);
        let input = cx.new(|cx| {
            let mut input = Input::field("Rename to-do", "To-do", content(0.35), cx);
            input.set_text(item.title.clone(), cx);
            input
        });
        self._subscriptions
            .push(cx.subscribe(&input, |this, _, _: &input::Submitted, cx| {
                this.commit_edit(cx)
            }));
        window.focus(&input.focus_handle(cx), cx);
        self.editing = Some((item.id.clone(), input));
        cx.notify();
    }

    fn commit_edit(&mut self, cx: &mut Context<Self>) {
        let Some((id, input)) = self.editing.take() else {
            return;
        };
        let title = input.read(cx).content.to_string();
        cx.notify();
        let unchanged = self
            .items
            .iter()
            .any(|item| item.id == id && item.title == title.trim());
        if title.trim().is_empty() || unchanged {
            return;
        }
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.title = title.trim().into();
        }
        self.change(
            move |store, board| store.rename_todo(board, &id, &title).map(|_| ()),
            cx,
        );
    }

    fn status_mark(status: TodoStatus) -> gpui::AnyElement {
        match status {
            TodoStatus::Todo => div()
                .size(px(13.))
                .rounded_full()
                .border_1()
                .border_color(content(0.4))
                .into_any_element(),
            TodoStatus::InProgress => icon("circle-dot", 15.)
                .text_color(accent_color())
                .into_any_element(),
            TodoStatus::Done => icon("check-circle", 15.)
                .text_color(content(0.45))
                .into_any_element(),
        }
    }

    fn row(&self, item: &TodoItem, cx: &mut Context<Self>) -> gpui::AnyElement {
        let id = item.id.clone();
        let status = item.status;
        let board = self.board.as_ref().map(TodoBoard::key).unwrap_or_default();
        let editing = self
            .editing
            .as_ref()
            .filter(|(editing, _)| *editing == item.id)
            .map(|(_, input)| input.clone());
        let done = status == TodoStatus::Done;
        let dragged = DraggedTodo {
            board,
            id: id.clone(),
            title: item.title.clone().into(),
        };
        let before = id.clone();
        div()
            .id(SharedString::from(format!("todo-{}", item.id)))
            .debug_selector({
                let id = item.id.clone();
                move || format!("todo-{id}")
            })
            .group("todo-row")
            .role(Role::ListItem)
            .aria_label(SharedString::from(format!(
                "{}, {}",
                item.title,
                status.label()
            )))
            .min_h(px(30.))
            .px_1()
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .hover(|s| s.bg(content(0.05)))
            .drag_over::<DraggedTodo>(|s, _, _, _| s.bg(content(0.08)))
            .on_drop(cx.listener(move |this, dragged: &DraggedTodo, _, cx| {
                this.move_to(dragged, status, Some(before.clone()), cx)
            }))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .child(
                div()
                    .id(SharedString::from(format!("todo-status-{}", item.id)))
                    .debug_selector({
                        let id = item.id.clone();
                        move || format!("todo-status-{id}")
                    })
                    .role(Role::Button)
                    .aria_label(SharedString::from(format!(
                        "Move to {}",
                        status.next().label()
                    )))
                    .size(px(22.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor(CursorStyle::PointingHand)
                    .hover(|s| s.bg(content(0.08)))
                    .child(Self::status_mark(status))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.advance(id.clone(), status, cx)
                        }
                    })),
            )
            .child(match editing {
                Some(input) => div().flex_1().min_w_0().child(input).into_any_element(),
                None => div()
                    .id(SharedString::from(format!("todo-title-{}", item.id)))
                    .flex_1()
                    .min_w_0()
                    .py_1()
                    .text_size(px(13.))
                    .text_color(content(if done { 0.4 } else { 0.85 }))
                    .when(done, |d| d.line_through())
                    .child(item.title.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let item = item.clone();
                            move |this, event: &gpui::MouseDownEvent, window, cx| {
                                if event.click_count == 2 {
                                    this.begin_edit(&item, window, cx)
                                }
                            }
                        }),
                    )
                    .into_any_element(),
            })
            .child(
                div()
                    .id(SharedString::from(format!("todo-delete-{}", item.id)))
                    .role(Role::Button)
                    .aria_label("Delete to-do")
                    .size(px(20.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .opacity(0.)
                    .group_hover("todo-row", |s| s.opacity(1.))
                    .hover(|s| s.bg(content(0.08)))
                    .child(icon("close", 12.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete(id.clone(), cx)
                    })),
            )
            .into_any_element()
    }

    fn column(&self, status: TodoStatus, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let items: Vec<_> = self
            .items
            .iter()
            .filter(|item| item.status == status)
            .cloned()
            .collect();
        let count = items.len();
        div()
            .id(SharedString::from(format!("todo-column-{}", status.id())))
            .debug_selector(move || format!("todo-column-{}", status.id()))
            .role(Role::List)
            .aria_label(status.label())
            .flex()
            .flex_col()
            .gap_px()
            .min_w_0()
            .min_h(px(64.))
            .rounded_lg()
            .drag_over::<DraggedTodo>(|s, _, _, _| s.bg(content(0.04)))
            .on_drop(cx.listener(move |this, dragged: &DraggedTodo, _, cx| {
                this.move_to(dragged, status, None, cx)
            }))
            .child(
                div()
                    .h(px(26.))
                    .px_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.5))
                    .child(status.label())
                    .child(div().text_color(content(0.3)).child(count.to_string())),
            )
            .children(items.iter().map(|item| self.row(item, cx)))
            .when(count == 0, |d| {
                d.child(
                    div()
                        .px_1()
                        .py_1()
                        .text_size(px(12.))
                        .text_color(content(0.3))
                        .child(match status {
                            TodoStatus::Todo => "Nothing to do",
                            TodoStatus::InProgress => "Nothing in progress",
                            TodoStatus::Done => "Nothing done yet",
                        }),
                )
            })
    }
}

impl Render for TodoBoardView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = self.columns;
        let body = div()
            .id("todo-board-columns")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .pb_2()
            .map(|d| {
                if columns {
                    d.flex().items_start().gap_3()
                } else {
                    d.flex().flex_col().gap_2()
                }
            })
            .children(TodoStatus::ALL.into_iter().map(|status| {
                self.column(status, cx).when(columns, |d| {
                    d.flex_1()
                        .p_2()
                        .bg(content(0.02))
                        .border_1()
                        .border_color(content(0.05))
                })
            }));
        div()
            .id("todo-board")
            .debug_selector(|| "todo-board".into())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.))
                    .flex_shrink_0()
                    .px_2()
                    .mx_2()
                    .my_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.08))
                    .child(icon("plus", 12.))
                    .child(div().flex_1().min_w_0().child(self.draft.clone())),
            )
            .children(
                self.change_error
                    .clone()
                    .or(self.error.clone())
                    .map(|error| {
                        div()
                            .px_3()
                            .pb_2()
                            .text_size(px(12.))
                            .text_color(gpui::rgb(0xe5484d))
                            .child(error)
                    }),
            )
            .child(body)
    }
}

/// A board the To-dos page can show.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BoardChoice {
    pub board: TodoBoard,
    pub label: String,
}

impl BoardChoice {
    fn group(&self) -> &'static str {
        match self.board {
            TodoBoard::Personal => "",
            TodoBoard::Project(_) => "Projects",
            TodoBoard::Bot(_) => "Bots",
        }
    }
}

pub(crate) enum PageEvent {
    Close,
}

/// The left sidebar's To-dos page: the personal board, or any project's or
/// Bot's, picked from a dropdown.
pub(crate) struct TodosPage {
    store: Database,
    board: Entity<TodoBoardView>,
    choices: Vec<BoardChoice>,
    open_counts: std::collections::HashMap<String, usize>,
    anchor: AnchorSlot,
    picker_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl gpui::EventEmitter<PageEvent> for TodosPage {}
impl Focusable for TodosPage {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.board.focus_handle(cx)
    }
}

impl TodosPage {
    pub(crate) fn new(store: Database, cx: &mut Context<Self>) -> Self {
        let board = cx.new(|cx| {
            let mut view = TodoBoardView::new(store.clone(), true, cx);
            view.set_board(Some(TodoBoard::Personal), cx);
            view
        });
        let subscriptions =
            vec![cx.observe_global::<TodoRevision>(|this, cx| this.load_counts(cx))];
        let mut page = Self {
            store,
            board,
            choices: vec![BoardChoice {
                board: TodoBoard::Personal,
                label: "Personal".into(),
            }],
            open_counts: Default::default(),
            anchor: AnchorSlot::default(),
            picker_open: false,
            _subscriptions: subscriptions,
        };
        page.load_counts(cx);
        page
    }

    /// Projects and Bots, after the personal board. A board that disappears
    /// falls back to the personal one.
    pub(crate) fn set_choices(&mut self, mut choices: Vec<BoardChoice>, cx: &mut Context<Self>) {
        choices.retain(|choice| choice.board != TodoBoard::Personal);
        choices.insert(
            0,
            BoardChoice {
                board: TodoBoard::Personal,
                label: "Personal".into(),
            },
        );
        let current = self.board.read(cx).board().cloned();
        if current.is_some_and(|board| !choices.iter().any(|c| c.board == board)) {
            self.select(TodoBoard::Personal, cx);
        }
        self.choices = choices;
        cx.notify();
    }

    pub(crate) fn select(&mut self, board: TodoBoard, cx: &mut Context<Self>) {
        self.picker_open = false;
        self.board
            .update(cx, |view, cx| view.set_board(Some(board), cx));
        cx.notify();
    }

    pub(crate) fn selected(&self, cx: &App) -> Option<TodoBoard> {
        self.board.read(cx).board().cloned()
    }

    fn load_counts(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.open_todo_counts() });
        cx.spawn(async move |this, cx| {
            if let Ok(counts) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.open_counts = counts;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn label(&self, cx: &App) -> String {
        let board = self.selected(cx);
        self.choices
            .iter()
            .find(|choice| Some(&choice.board) == board.as_ref())
            .map(|choice| choice.label.clone())
            .unwrap_or_else(|| "Personal".into())
    }

    fn picker(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let weak = cx.weak_entity();
        let selected = self.selected(cx);
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let mut group = "";
        for (ix, choice) in self.choices.iter().enumerate() {
            if choice.group() != group {
                group = choice.group();
                rows.push(
                    div()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(group)
                        .into_any_element(),
                );
            }
            let board = choice.board.clone();
            let count = self.open_counts.get(&board.key()).copied().unwrap_or(0);
            let current = selected.as_ref() == Some(&board);
            rows.push(
                div()
                    .id(("todo-board-choice", ix))
                    .debug_selector(move || format!("todo-board-choice-{ix}"))
                    .role(Role::MenuItemRadio)
                    .aria_label(SharedString::from(choice.label.clone()))
                    .aria_selected(current)
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .when(current, |d| d.bg(content(0.08)))
                    .hover(|s| s.bg(content(0.08)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(choice.label.clone()),
                    )
                    .when(count > 0, |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(content(0.4))
                                .child(count.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select(board.clone(), cx)))
                    .into_any_element(),
            );
        }
        popover("todo-board-picker", Anchor::Slot(self.anchor.clone()))
            .width(260.)
            .max_height(380.)
            .on_dismiss(move |_, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.picker_open = false;
                    cx.notify();
                });
            })
            .child(
                div()
                    .id("todo-board-choices")
                    .p_1()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}

impl Render for TodosPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = self.label(cx);
        div()
            .id("todos-page")
            .debug_selector(|| "todos-page".into())
            .role(Role::Region)
            .aria_label("To-dos")
            .size_full()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    if this.picker_open {
                        this.picker_open = false;
                        cx.notify();
                    } else {
                        cx.emit(PageEvent::Close);
                    }
                }
            }))
            .child(
                div()
                    .h(px(44.))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("todo-board-picker-button")
                            .debug_selector(|| "todo-board-picker-button".into())
                            .role(Role::Button)
                            .aria_label("Choose board")
                            .relative()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .hover(|s| s.bg(content(0.08)))
                            .when(self.picker_open, |d| d.bg(content(0.08)))
                            .child(self.anchor.probe())
                            .child(label)
                            .child(icon("chevron-down", 11.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.picker_open = !this.picker_open;
                                if this.picker_open {
                                    this.load_counts(cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.board.clone()))
            .when(self.picker_open, |d| d.child(self.picker(cx)))
    }
}

#[cfg(test)]
mod tests {
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
}
