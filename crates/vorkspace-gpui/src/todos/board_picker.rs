//! Board selection, counts and legacy task assignment.
use super::{TodoBoardView, TodoRevision, changed, labeled_secondary};
use crate::{
    controls::page,
    popover::{Anchor, AnchorSlot, popover},
};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, Role, SharedString, Subscription,
    Task, Window, div, prelude::*, px,
};
use std::collections::HashMap;
use vorkspace_core::todos::TodoBoard;
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::{radius, theme::*};

/// A board the To-dos page can show.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BoardChoice {
    pub board: TodoBoard,
    pub label: String,
}

#[derive(Clone)]
struct LegacyTasks {
    id: String,
    label: String,
    count: usize,
    projects: Vec<String>,
}

impl BoardChoice {
    fn group(&self) -> &'static str {
        match self.board {
            TodoBoard::Personal => "",
            TodoBoard::Project(_) => "Projects",
            TodoBoard::Bot(_) => "",
        }
    }
}

pub(crate) enum PageEvent {
    Close,
}

/// The left sidebar's To-dos page: the personal board or a project board,
/// picked from a dropdown.
#[cfg(test)]
impl TodosPage {
    pub(crate) fn board_for_capture(&self) -> Entity<TodoBoardView> {
        self.board.clone()
    }
}

pub(crate) struct TodosPage {
    store: Database,
    board: Entity<TodoBoardView>,
    choices: Vec<BoardChoice>,
    open_counts: HashMap<String, usize>,
    legacy: Vec<LegacyTasks>,
    assign_error: Option<String>,
    anchor: AnchorSlot,
    picker_open: bool,
    counts_task: Option<Task<()>>,
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
            legacy: Vec::new(),
            assign_error: None,
            anchor: AnchorSlot::default(),
            picker_open: false,
            counts_task: None,
            _subscriptions: subscriptions,
        };
        page.load_counts(cx);
        page
    }

    /// Projects, after the personal board. A board that disappears
    /// falls back to the personal one.
    pub(crate) fn set_choices(&mut self, mut choices: Vec<BoardChoice>, cx: &mut Context<Self>) {
        choices.retain(|choice| matches!(choice.board, TodoBoard::Project(_)));
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
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            let counts = store.open_todo_counts()?;
            let legacy = store.legacy_bot_todo_counts()?;
            let roster = vorkspace_core::bot::Roster::load(&store).ok();
            let legacy = legacy
                .into_iter()
                .map(|(id, count)| {
                    let bot = roster.as_ref().and_then(|roster| roster.get(&id));
                    LegacyTasks {
                        label: bot
                            .map(|bot| bot.name.clone())
                            .unwrap_or_else(|| format!("Former Bot {id}")),
                        projects: bot.map(|bot| bot.projects.clone()).unwrap_or_default(),
                        id,
                        count,
                    }
                })
                .collect::<Vec<_>>();
            Ok::<_, String>((counts, legacy))
        });
        self.counts_task = Some(cx.spawn(async move |this, cx| {
            if let Ok((counts, legacy)) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.open_counts = counts;
                    this.legacy = legacy;
                    cx.notify();
                });
            }
        }));
    }

    fn assign_legacy(&mut self, id: String, project: String, cx: &mut Context<Self>) {
        self.assign_error = None;
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.assign_legacy_bot_todos(&id, &project) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        changed(cx);
                        this.load_counts(cx);
                    }
                    Err(error) => this.assign_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn legacy_panel(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let Some(TodoBoard::Project(project)) = self.selected(cx) else {
            return None;
        };
        let rows = self
            .legacy
            .iter()
            .filter(|legacy| {
                legacy.projects.is_empty()
                    || legacy.projects.iter().any(|path| {
                        vorkspace_core::files::path_key(path)
                            == vorkspace_core::files::path_key(&project)
                    })
            })
            .enumerate()
            .map(|(ix, legacy)| {
                let id = legacy.id.clone();
                let project = project.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(format!("{} · {} saved task(s)", legacy.label, legacy.count)),
                    )
                    .child(
                        labeled_secondary(("todo-assign-legacy", ix), "Move here", false, false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.assign_legacy(id.clone(), project.clone(), cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return None;
        }
        Some(
            div()
                .mx_3()
                .mt_2()
                .p_2()
                .rounded(px(radius::CONTROL))
                .border_1()
                .border_color(page::divider())
                .flex()
                .flex_col()
                .gap_2()
                .text_size(px(page::META_SIZE))
                .child("Saved Bot tasks need a project")
                .children(rows)
                .children(
                    self.assign_error
                        .clone()
                        .map(|error| div().text_color(vorkspace_ui::theme::danger()).child(error)),
                )
                .into_any_element(),
        )
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
                        .text_size(px(page::META_SIZE))
                        .text_color(page::secondary_ink())
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
                    .rounded(px(radius::CONTROL))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .when(current, |d| d.bg(content(0.08)))
                    .hover(|s| s.bg(vorkspace_ui::fluid::hover(0.08)))
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
                                .text_size(px(page::META_SIZE))
                                .text_color(page::secondary_ink())
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
                page::toolbar().child(
                    page::action("todo-board-picker-button", "Choose board", true)
                        .debug_selector(|| "todo-board-picker-button".into())
                        .relative()
                        .text_size(px(page::LABEL_SIZE))
                        .font_weight(FontWeight::SEMIBOLD)
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
            .when_some(self.legacy_panel(cx), |d, panel| d.child(panel))
            .child(div().flex_1().min_h_0().child(self.board.clone()))
            .when(self.picker_open, |d| d.child(self.picker(cx)))
    }
}
