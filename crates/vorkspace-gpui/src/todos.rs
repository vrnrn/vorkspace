//! Project and personal to-do boards. A project may also show its GitHub
//! issues; each local or linked item retains its own authority.
use crate::{
    controls::page,
    input::{self, Input},
    popover::{Anchor, AnchorSlot, popover},
};
use gpui::{
    App, Context, Div, ElementId, Entity, FocusHandle, Focusable, FontWeight, Global, MouseButton,
    Role, SharedString, Stateful, Subscription, Window, div, prelude::*, px,
};
use std::borrow::Cow;
use std::collections::HashMap;
use vorkspace_core::todos::{TodoBoard, TodoItem, TodoStatus, TodoSync};
use vorkspace_runtime::runtime::{Database, locked};
use vorkspace_ui::radius;
use vorkspace_ui::theme::*;
mod cards;
mod creation;
mod details;
mod service;
pub(crate) use service::action as todo_action;

const STALE_DETAILS: &str = "This to-do changed while you were editing. Your draft is preserved; reload details before saving.";

/// Bumped after any board changes, so every open view of it reloads.
#[derive(Default)]
pub(crate) struct TodoRevision(pub u64);
impl Global for TodoRevision {}

pub(crate) fn changed(cx: &mut App) {
    let next = cx.try_global::<TodoRevision>().map_or(0, |r| r.0) + 1;
    cx.set_global(TodoRevision(next));
}

fn labeled_primary(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Stateful<Div> {
    let label = label.into();
    page::primary(id, label.clone(), disabled).child(label)
}

fn labeled_secondary(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    danger: bool,
    disabled: bool,
) -> Stateful<Div> {
    let label = label.into();
    page::secondary(id, label.clone(), danger, disabled).child(label)
}

type ChangeWork = Box<dyn FnOnce(&Database, &TodoBoard) -> Result<(), String> + Send>;

/// A store change waiting its turn. A view's changes run one at a time, in
/// the order they were made, so a later click can't be overwritten by an
/// earlier one that reached the database second.
struct Change {
    board: TodoBoard,
    work: ChangeWork,
    /// A submitted draft to put back if the change fails.
    draft: Option<String>,
    saved_details: Option<SavedDetails>,
}

struct SavedDetails {
    id: String,
}

#[derive(Clone)]
struct DetailsEditor {
    /// Original card and draft key. A linked issue can change displayed IDs.
    id: String,
    issue: Option<service::IssueIdentity>,
    revision: i64,
    dependencies_complete: bool,
    title: Entity<Input>,
    description: Entity<Input>,
    attachments: Entity<Input>,
    dependencies: Entity<Input>,
    url: Entity<Input>,
    preview: Entity<crate::markdown::Document>,
}

impl DetailsEditor {
    fn matches_revision(&self, item: &TodoItem) -> bool {
        // Local receipt revisions and GitHub timestamps have different owners.
        self.id == item.id
            && self.revision == item.updated_at
            && self
                .issue
                .as_ref()
                .map_or(item.issue.is_none(), |issue| issue.matches(item))
    }
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
            .rounded(px(radius::CONTROL))
            .bg(pane_fill())
            .border_1()
            .border_color(page::divider())
            .text_size(px(page::LABEL_SIZE))
            .text_color(content(0.9))
            .truncate()
            .child(self.title.clone())
    }
}

pub(crate) struct TodoBoardView {
    store: Database,
    board: Option<TodoBoard>,
    items: Vec<TodoItem>,
    published_receipts: HashMap<String, service::IssueIdentity>,
    /// Guards against an older load landing after a newer one.
    generation: u64,
    /// The last load's failure.
    error: Option<String>,
    github_notice: Option<String>,
    github_repo: Option<String>,
    /// The last change's failure; a reload doesn't clear it.
    change_error: Option<String>,
    board_errors: std::collections::HashMap<String, String>,
    changes: async_channel::Sender<Change>,
    /// Changes sent and not yet finished. Loads wait for them to finish, so
    /// a reload between two changes can't show the first undoing the second.
    pending_changes: usize,
    /// Side-by-side columns; otherwise stacked.
    columns: bool,
    draft: Entity<Input>,
    create_open: bool,
    create_anchor: AnchorSlot,
    create_focus: FocusHandle,
    detail_focus: FocusHandle,
    detail_return_focus: Option<FocusHandle>,
    detail_editing: bool,
    details: Option<DetailsEditor>,
    saved_details: std::collections::HashMap<String, DetailsEditor>,
    show_dependency_choices: bool,
    confirm_retry: bool,
    confirm_discard: bool,
    confirm_cancel: bool,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for TodoBoardView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.create_focus.clone()
    }
}

impl TodoBoardView {
    pub(crate) fn new(store: Database, columns: bool, cx: &mut Context<Self>) -> Self {
        let draft =
            cx.new(|cx| Input::field("Task title", "Task title", page::secondary_ink(), cx));
        let subscriptions = vec![
            cx.subscribe(&draft, |this, _, _: &input::Submitted, cx| this.add(cx)),
            cx.observe_global::<TodoRevision>(|this, cx| this.refresh(cx)),
        ];
        let (changes, queue) = async_channel::unbounded::<Change>();
        // Queued changes still run if the view closes; the queue ends once
        // it is empty and the view is gone.
        let worker_store = store.clone();
        cx.spawn(async move |this, cx| {
            while let Ok(Change {
                board,
                work,
                draft,
                saved_details,
            }) = queue.recv().await
            {
                let store = worker_store.clone();
                let changed_board = board.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { work(&store, &board) })
                    .await;
                let idle = queue.is_empty();
                cx.update(|cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.finish_change(result, draft, &changed_board, saved_details, cx)
                    });
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
            published_receipts: HashMap::new(),
            generation: 0,
            error: None,
            github_notice: None,
            github_repo: None,
            change_error: None,
            board_errors: Default::default(),
            changes,
            pending_changes: 0,
            columns,
            draft,
            create_open: false,
            create_anchor: AnchorSlot::default(),
            create_focus: cx.focus_handle(),
            detail_focus: cx.focus_handle(),
            detail_return_focus: None,
            detail_editing: false,
            details: None,
            saved_details: Default::default(),
            show_dependency_choices: false,
            confirm_retry: false,
            confirm_discard: false,
            confirm_cancel: false,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn board(&self) -> Option<&TodoBoard> {
        self.board.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn items(&self) -> &[TodoItem] {
        &self.items
    }

    pub(crate) fn set_board(&mut self, board: Option<TodoBoard>, cx: &mut Context<Self>) {
        if self.board == board {
            return;
        }
        self.stash_details();
        self.create_open = false;
        if let Some(error) = self.change_error.take()
            && let Some(old_board) = self.board.as_ref()
        {
            self.board_errors.insert(old_board.key(), error);
        }
        self.board = board;
        self.items.clear();
        self.published_receipts.clear();
        self.error = None;
        self.github_notice = None;
        self.github_repo = None;
        self.change_error = self
            .board
            .as_ref()
            .and_then(|board| self.board_errors.remove(&board.key()));
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
            .spawn(async move { service::list(&store, &board) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(snapshot) => this.install_snapshot(snapshot),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install_snapshot(&mut self, snapshot: service::BoardSnapshot) {
        self.items = snapshot.items;
        self.published_receipts = snapshot.published_receipts;
        self.error = snapshot.github_error;
        self.github_notice = snapshot.github_notice;
        self.github_repo = snapshot.github_repo;
    }

    /// Queues a store change to run off the main thread after this view's
    /// earlier ones; every view reloads once the queue is empty.
    fn change(
        &mut self,
        work: impl FnOnce(&Database, &TodoBoard) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.queue_change(work, None, None, cx);
    }

    fn queue_change(
        &mut self,
        work: impl FnOnce(&Database, &TodoBoard) -> Result<(), String> + Send + 'static,
        draft: Option<String>,
        saved_details: Option<SavedDetails>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(board) = self.board.clone() else {
            return false;
        };
        let change = Change {
            board,
            work: Box::new(work),
            draft,
            saved_details,
        };
        if self.changes.try_send(change).is_err() {
            return false;
        }
        self.pending_changes += 1;
        // A load already under way predates this change.
        self.generation += 1;
        self.change_error = None;
        cx.notify();
        true
    }

    fn finish_change(
        &mut self,
        result: Result<(), String>,
        draft: Option<String>,
        board: &TodoBoard,
        saved_details: Option<SavedDetails>,
        cx: &mut Context<Self>,
    ) {
        self.pending_changes = self.pending_changes.saturating_sub(1);
        if let Some(saved) = saved_details {
            let key = format!("{}\0{}", board.key(), saved.id);
            if result.is_ok() {
                if self.board.as_ref() == Some(board)
                    && self
                        .details
                        .as_ref()
                        .is_some_and(|editor| editor.id == saved.id)
                {
                    self.details = None;
                }
                self.saved_details.remove(&key);
                cx.notify();
            } else {
                let editor = self
                    .details
                    .as_ref()
                    .filter(|editor| self.board.as_ref() == Some(board) && editor.id == saved.id)
                    .or_else(|| self.saved_details.get(&key));
                if let Some(editor) = editor {
                    Self::set_details_read_only(editor, false, cx);
                }
            }
        }
        if let Err(error) = result {
            if self.board.as_ref() == Some(board) {
                self.change_error = Some(error);
            } else {
                self.board_errors.insert(board.key(), error);
            }
            // Put the submission back unless something newer was typed.
            if let Some(title) = draft
                && self.draft.read(cx).content.is_empty()
            {
                self.draft.update(cx, |input, cx| input.set_text(title, cx));
            }
            cx.notify();
        }
    }

    fn set_details_read_only(editor: &DetailsEditor, read_only: bool, cx: &mut Context<Self>) {
        for input in [
            &editor.title,
            &editor.description,
            &editor.attachments,
            &editor.dependencies,
            &editor.url,
        ] {
            input.update(cx, |input, _| input.set_read_only(read_only));
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
        if self.queue_change(
            {
                let title = title.clone();
                move |store, board| {
                    service::action(
                        store,
                        board,
                        "todos.add",
                        &serde_json::json!({"title":title}),
                    )
                    .map(|_| ())
                }
            },
            Some(title),
            None,
            cx,
        ) {
            self.draft
                .update(cx, |draft, cx| draft.set_text(String::new(), cx));
            self.create_open = false;
        }
    }

    fn advance(&mut self, id: String, status: TodoStatus, cx: &mut Context<Self>) {
        if self.items.iter().any(|item| {
            item.id == id
                && (item.pending_edit.is_some()
                    || matches!(
                        item.sync,
                        vorkspace_core::todos::TodoSync::Pending
                            | vorkspace_core::todos::TodoSync::Uncertain
                    ))
        }) {
            return;
        }
        // Show the new status at once; the reload confirms it.
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.status = status.next();
            cx.notify();
        }
        self.change(
            move |store, board| {
                service::action(
                    store,
                    board,
                    "todos.update",
                    &serde_json::json!({"id":id,"status":status.next().id()}),
                )
                .map(|_| ())
            },
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
            || self.items.iter().any(|item| {
                item.id == dragged.id
                    && (item.pending_edit.is_some()
                        || matches!(
                            item.sync,
                            vorkspace_core::todos::TodoSync::Pending
                                | vorkspace_core::todos::TodoSync::Uncertain
                        ))
            })
        {
            return;
        }
        let id = dragged.id.clone();
        let remote = self
            .items
            .iter()
            .any(|item| item.id == id && item.issue.is_some());
        let before = if let Some(target) = before {
            let Some(at) = self
                .items
                .iter()
                .position(|item| item.id == target && item.status == status)
            else {
                return;
            };
            // GitHub cards have no SQLite ordering slot. Use the next local
            // card in the displayed column, or append if there is none.
            self.items[at..]
                .iter()
                .find(|item| item.status == status && item.issue.is_none() && item.id != id)
                .map(|item| item.id.clone())
        } else {
            None
        };
        self.change(
            move |store, board| {
                let mut input = serde_json::json!({"id":id,"status":status.id()});
                if !remote && let Some(before) = before {
                    input["before"] = before.into();
                }
                service::action(store, board, "todos.update", &input).map(|_| ())
            },
            cx,
        );
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        if self.items.iter().any(|item| {
            item.id == id
                && (item.pending_edit.is_some()
                    || matches!(
                        item.sync,
                        vorkspace_core::todos::TodoSync::Pending
                            | vorkspace_core::todos::TodoSync::Uncertain
                    ))
        }) {
            return;
        }
        let remote = self
            .items
            .iter()
            .any(|item| item.id == id && item.issue.is_some());
        if remote {
            if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                item.status = TodoStatus::Done;
            }
        } else {
            self.items.retain(|item| item.id != id);
        }
        cx.notify();
        self.change(
            move |store, board| {
                service::action(store, board, "todos.remove", &serde_json::json!({"id":id}))
                    .map(|_| ())
            },
            cx,
        );
    }

    fn details_key(&self, id: &str) -> Option<String> {
        Some(format!("{}\0{id}", self.board.as_ref()?.key()))
    }

    fn details_issue<'a>(
        &'a self,
        editor: &'a DetailsEditor,
    ) -> Option<&'a service::IssueIdentity> {
        editor
            .issue
            .as_ref()
            .or_else(|| self.published_receipts.get(&editor.id))
    }

    fn details_item(&self, editor: &DetailsEditor) -> Option<&TodoItem> {
        self.items
            .iter()
            .find(|item| item.id == editor.id)
            .or_else(|| {
                let issue = self.details_issue(editor)?;
                self.items.iter().find(|item| issue.matches(item))
            })
    }

    fn open_detail_item(&self) -> Option<&TodoItem> {
        self.details_item(self.details.as_ref()?)
    }

    fn saved_details_key(&self, item: &TodoItem) -> Option<String> {
        let key = self.details_key(&item.id)?;
        if self.saved_details.contains_key(&key) {
            return Some(key);
        }
        item.issue.as_ref()?;
        let prefix = self.details_key("")?;
        // Leave other drafts under their original keys if two card IDs were
        // editable before the publication receipt was visible to this view.
        self.saved_details
            .iter()
            .filter_map(|(key, editor)| {
                (key.starts_with(&prefix)
                    && self
                        .details_issue(editor)
                        .is_some_and(|issue| issue.matches(item)))
                .then_some(key)
            })
            .min()
            .cloned()
    }

    fn stash_details(&mut self) {
        if let Some(editor) = self.details.take()
            && let Some(key) = self.details_key(&editor.id)
        {
            self.saved_details.insert(key, editor);
        }
        self.show_dependency_choices = false;
        self.detail_editing = false;
    }

    fn open_details(&mut self, item: &TodoItem, cx: &mut Context<Self>) {
        if self.details.as_ref().is_some_and(|editor| {
            editor.id == item.id
                || self
                    .details_item(editor)
                    .is_some_and(|current| current.id == item.id)
        }) {
            return;
        }
        self.stash_details();
        if let Some(key) = self.saved_details_key(item)
            && let Some(mut editor) = self.saved_details.remove(&key)
        {
            let text = editor.description.read(cx).content.clone();
            editor.preview =
                cx.new(|cx| crate::markdown::Document::prose(&text, "No description", cx));
            self.details = Some(editor);
            cx.notify();
            return;
        }
        let title = cx.new(|cx| {
            let mut input = Input::field("To-do title", "Title", page::secondary_ink(), cx);
            input.set_text(
                item.pending_edit
                    .as_ref()
                    .and_then(|edit| edit.input.get("title").and_then(serde_json::Value::as_str))
                    .unwrap_or(&item.title)
                    .to_string(),
                cx,
            );
            input.select_range(0..0, cx);
            input
        });
        let description = cx.new(|cx| {
            let mut input = Input::multiline_field("To-do description", "Describe the work", cx);
            input.set_text(
                item.pending_edit
                    .as_ref()
                    .and_then(|edit| {
                        edit.input
                            .get("description")
                            .and_then(serde_json::Value::as_str)
                    })
                    .unwrap_or(&item.description)
                    .to_string(),
                cx,
            );
            input
        });
        let attachments = cx.new(|cx| {
            let mut input =
                Input::multiline_field("Attachments", "One project file path or URL per line", cx);
            let pending = item
                .pending_edit
                .as_ref()
                .and_then(|edit| {
                    edit.input
                        .get("attachments")
                        .and_then(serde_json::Value::as_array)
                })
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            if let Some(pending) = pending {
                input.set_text(pending, cx);
            } else if item.issue.is_none() {
                input.set_text(item.attachments.join("\n"), cx);
            }
            input
        });
        let dependencies = cx.new(|cx| {
            let mut input = Input::field(
                "Dependencies",
                "Issue numbers or local IDs, separated by commas",
                page::secondary_ink(),
                cx,
            );
            let pending = item
                .pending_edit
                .as_ref()
                .and_then(|edit| {
                    edit.input
                        .get("dependencies")
                        .and_then(serde_json::Value::as_array)
                })
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                });
            input.set_text(pending.unwrap_or_else(|| item.dependencies.join(", ")), cx);
            input
        });
        let url = cx.new(|cx| {
            Input::field(
                "Attachment URL",
                "Paste an HTTPS link",
                page::secondary_ink(),
                cx,
            )
        });
        self._subscriptions
            .push(cx.subscribe(&url, |this, _, _: &input::Submitted, cx| {
                this.add_attachment_url(cx)
            }));
        let text = description.read(cx).content.clone();
        let preview = cx.new(|cx| crate::markdown::Document::prose(&text, "No description", cx));
        self.details = Some(DetailsEditor {
            id: item.id.clone(),
            issue: service::IssueIdentity::from_item(item),
            revision: item.updated_at,
            dependencies_complete: !item
                .issue
                .as_ref()
                .is_some_and(|issue| issue.dependencies_truncated),
            title,
            description,
            attachments,
            dependencies,
            url,
            preview,
        });
        self.confirm_retry = false;
        self.confirm_discard = false;
        self.confirm_cancel = false;
        cx.notify();
    }

    fn reference_lines(input: &Entity<Input>, cx: &App) -> Vec<String> {
        input
            .read(cx)
            .content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn dependency_values(input: &Entity<Input>, cx: &App) -> Vec<String> {
        input
            .read(cx)
            .content
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn dependency_ref(item: &TodoItem) -> String {
        item.issue
            .as_ref()
            .map(|issue| Self::issue_dependency_ref(&issue.repo, issue.number))
            .unwrap_or_else(|| item.id.clone())
    }

    fn issue_dependency_ref(repo: &str, number: i64) -> String {
        format!(
            "https://github.com/{}/issues/{number}",
            repo.to_ascii_lowercase()
        )
    }

    fn dependency_key<'a>(&self, owner: &TodoItem, reference: &'a str) -> Cow<'a, str> {
        if let Some((repo, number)) = vorkspace_core::todos::github::issue_ref_from_url(reference) {
            return Cow::Owned(Self::issue_dependency_ref(&repo, number));
        }
        if let Some(number) = reference
            .strip_prefix('#')
            .and_then(|number| number.parse::<i64>().ok())
            .filter(|number| *number > 0)
            && let Some(repo) = owner
                .issue
                .as_ref()
                .map(|issue| issue.repo.as_str())
                .or(self.github_repo.as_deref())
        {
            return Cow::Owned(Self::issue_dependency_ref(repo, number));
        }
        Cow::Borrowed(reference)
    }

    fn dependency_item_label(item: &TodoItem) -> String {
        if let Some(issue) = &item.issue {
            format!("{}#{} · {}", issue.repo, issue.number, item.title)
        } else {
            item.title.clone()
        }
    }

    fn dependency_label(&self, owner: &TodoItem, reference: &str) -> String {
        self.dependency_item(owner, reference)
            .map(Self::dependency_item_label)
            .unwrap_or_else(|| {
                if let Some(path) = reference.strip_prefix("https://github.com/") {
                    format!("GitHub issue {path}")
                } else if reference.starts_with("https://") {
                    reference.to_string()
                } else if reference.starts_with('#') {
                    format!("Issue {reference}")
                } else {
                    "Unavailable task".into()
                }
            })
    }

    fn toggle_dependency(&mut self, reference: String, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        let Some(editor) = self.details.as_ref() else {
            return;
        };
        let Some(item) = self.details_item(editor) else {
            return;
        };
        if item
            .issue
            .as_ref()
            .is_some_and(|issue| issue.dependencies_truncated)
        {
            return;
        }
        let mut values = Self::dependency_values(&editor.dependencies, cx);
        let key = self.dependency_key(item, &reference);
        let previous_count = values.len();
        values.retain(|value| self.dependency_key(item, value) != key);
        if values.len() == previous_count {
            values.push(reference);
        }
        editor
            .dependencies
            .update(cx, |input, cx| input.set_text(values.join(", "), cx));
        cx.notify();
    }

    fn append_attachment(&mut self, reference: String, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        let Some(editor) = self.details.as_ref() else {
            return;
        };
        let mut values = Self::reference_lines(&editor.attachments, cx);
        if !values.contains(&reference) {
            values.push(reference);
            editor
                .attachments
                .update(cx, |input, cx| input.set_text(values.join("\n"), cx));
            cx.notify();
        }
    }

    fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        let Some(editor) = self.details.as_ref() else {
            return;
        };
        let mut values = Self::reference_lines(&editor.attachments, cx);
        if index < values.len() {
            values.remove(index);
            editor
                .attachments
                .update(cx, |input, cx| input.set_text(values.join("\n"), cx));
            cx.notify();
        }
    }

    fn add_attachment_url(&mut self, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        let Some(editor) = self.details.as_ref() else {
            return;
        };
        let value = editor.url.read(cx).content.trim().to_string();
        if !value.starts_with("https://") || value.chars().any(char::is_whitespace) {
            self.change_error = Some("Use a complete HTTPS attachment URL".into());
            cx.notify();
            return;
        }
        editor
            .url
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.append_attachment(value, cx);
    }

    fn add_attachment_files(&mut self, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        let Some(editor) = self.details.as_ref() else {
            return;
        };
        let id = editor.id.clone();
        let board = self.board.clone();
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add task attachments".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                let _ = this.update(cx, |this, cx| {
                    if this.pending_changes == 0
                        && this.board == board
                        && this.details.as_ref().is_some_and(|d| d.id == id)
                    {
                        for path in paths {
                            this.append_attachment(path.to_string_lossy().into_owned(), cx);
                        }
                    }
                });
            }
        })
        .detach();
    }

    fn save_details(&mut self, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        let Some(details) = self.details.as_ref() else {
            return;
        };
        let Some(item) = self.details_item(details) else {
            return;
        };
        if item.pending_edit.is_some()
            || matches!(item.sync, TodoSync::Pending | TodoSync::Uncertain)
        {
            self.change_error =
                Some("Reconcile the pending GitHub change before editing this to-do".into());
            cx.notify();
            return;
        }
        let remote = item.issue.is_some();
        if !details.matches_revision(item) {
            self.change_error = Some(STALE_DETAILS.into());
            cx.notify();
            return;
        }
        if !details.url.read(cx).content.trim().is_empty() {
            self.change_error =
                Some("Add or clear the unfinished attachment link before saving.".into());
            cx.notify();
            return;
        }
        let title = details.title.read(cx).content.to_string();
        let description = details.description.read(cx).content.to_string();
        let attachments = Self::reference_lines(&details.attachments, cx);
        let mut input = serde_json::json!({
            "id":item.id,"title":title,"description":description,"expectedUpdatedAt":details.revision
        });
        if details.dependencies_complete {
            input["dependencies"] =
                serde_json::json!(Self::dependency_values(&details.dependencies, cx));
        }
        if !remote || !attachments.is_empty() {
            input["attachments"] = serde_json::json!(attachments);
        }
        self.queue_detail_change(details.id.clone(), "todos.update", input, cx);
    }

    fn publish_details(&mut self, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        let Some(details) = self.details.as_ref() else {
            return;
        };
        let Some(item) = self.details_item(details) else {
            return;
        };
        if item.sync != TodoSync::Local || item.issue.is_some() {
            return;
        }
        if !details.matches_revision(item) {
            self.change_error = Some("This to-do changed while you were editing. Your draft is preserved; reload details before publishing.".into());
            cx.notify();
            return;
        }
        let unsaved = if !details.url.read(cx).content.trim().is_empty() {
            Some("Add or clear the unfinished attachment link before publishing.")
        } else if details.title.read(cx).content.as_ref() != item.title.as_str()
            || details.description.read(cx).content.as_ref() != item.description.as_str()
            || Self::reference_lines(&details.attachments, cx) != item.attachments
            || Self::dependency_values(&details.dependencies, cx) != item.dependencies
        {
            Some("Save your changes before publishing this to-do.")
        } else {
            None
        };
        if let Some(reason) = unsaved {
            self.change_error = Some(reason.into());
            cx.notify();
            return;
        }
        let id = details.id.clone();
        let input = serde_json::json!({"id":id,"expectedUpdatedAt":details.revision});
        self.queue_detail_change(id, "todos.publish", input, cx);
    }

    fn queue_detail_change(
        &mut self,
        id: String,
        action: &'static str,
        input: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let queued = self.queue_change(
            move |store, board| service::action(store, board, action, &input).map(|_| ()),
            None,
            Some(SavedDetails { id }),
            cx,
        );
        if queued && let Some(editor) = self.details.as_ref() {
            Self::set_details_read_only(editor, true, cx);
        }
    }

    fn retry_detail(&mut self, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        if !self.confirm_retry {
            self.confirm_retry = true;
            cx.notify();
            return;
        }
        let Some(item) = self.open_detail_item() else {
            return;
        };
        let id = item.id.clone();
        let edit = item.pending_edit.is_some();
        self.confirm_retry = false;
        self.change(
            move |store, board| {
                let (action, input) = if edit {
                    (
                        "todos.retryEdit",
                        serde_json::json!({"id":id,"confirmNoEffect":true}),
                    )
                } else {
                    (
                        "todos.retry",
                        serde_json::json!({"id":id,"confirmNoIssue":true}),
                    )
                };
                service::action(store, board, action, &input).map(|_| ())
            },
            cx,
        );
    }

    fn reconcile_detail(&mut self, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        let Some(item) = self.open_detail_item() else {
            return;
        };
        let id = item.id.clone();
        let edit = item.pending_edit.is_some();
        self.change(
            move |store, board| {
                service::action(
                    store,
                    board,
                    if edit {
                        "todos.reconcileEdit"
                    } else {
                        "todos.reconcile"
                    },
                    &serde_json::json!({"id":id}),
                )
                .map(|_| ())
            },
            cx,
        );
    }

    fn discard_detail(&mut self, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        if !self.confirm_discard {
            self.confirm_discard = true;
            cx.notify();
            return;
        }
        let Some(item) = self.open_detail_item() else {
            return;
        };
        let id = item.id.clone();
        self.confirm_discard = false;
        self.change(
            move |store, board| {
                service::action(
                    store,
                    board,
                    "todos.discardEdit",
                    &serde_json::json!({"id":id,"confirmChecked":true}),
                )
                .map(|_| ())
            },
            cx,
        );
    }

    fn cancel_publish_detail(&mut self, cx: &mut Context<Self>) {
        if self.pending_changes > 0 {
            return;
        }
        if !self.confirm_cancel {
            self.confirm_cancel = true;
            cx.notify();
            return;
        }
        let Some(item) = self.open_detail_item() else {
            return;
        };
        let id = item.id.clone();
        self.confirm_cancel = false;
        self.change(
            move |store, board| {
                service::action(
                    store,
                    board,
                    "todos.cancelPublish",
                    &serde_json::json!({"id":id,"confirmNoIssue":true}),
                )
                .map(|_| ())
            },
            cx,
        );
    }

    fn editor_panel(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let editor = self.details.as_ref()?;
        let item = self.details_item(editor)?;
        let remote = item.issue.is_some();
        let issue = item.issue.clone();
        let can_publish = matches!(self.board, Some(TodoBoard::Project(_)))
            && self.github_repo.is_some()
            && item.sync == vorkspace_core::todos::TodoSync::Local;
        let uncertain = matches!(
            item.sync,
            vorkspace_core::todos::TodoSync::Pending | vorkspace_core::todos::TodoSync::Uncertain
        );
        let pending_edit = item.pending_edit.is_some();
        let dependencies_truncated = item
            .issue
            .as_ref()
            .is_some_and(|issue| issue.dependencies_truncated);
        let attachment_refs = Self::reference_lines(&editor.attachments, cx);
        let selected_dependencies = Self::dependency_values(&editor.dependencies, cx);
        let selected_keys = selected_dependencies
            .iter()
            .map(|reference| self.dependency_key(item, reference))
            .collect::<std::collections::HashSet<_>>();
        let own_reference = Self::dependency_ref(item);
        let dependency_choices = self
            .items
            .iter()
            .filter(|candidate| !remote || candidate.issue.is_some())
            .filter_map(|candidate| {
                let reference = Self::dependency_ref(candidate);
                let selected = selected_keys.contains(reference.as_str());
                (reference != own_reference)
                    .then(|| (reference, Self::dependency_item_label(candidate), selected))
            })
            .collect::<Vec<_>>();
        Some(
            div()
                .id("todo-details")
                .debug_selector(|| "todo-details".into())
                .flex_1()
                .w_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(52.))
                        .px_3()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .border_b_1()
                        .border_color(page::divider())
                        .child(
                            labeled_secondary("todo-details-close", "Close", false, false)
                                .debug_selector(|| "todo-details-close".into())
                                .flex_shrink_0()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.dismiss_details(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(page::META_SIZE))
                                .text_color(page::secondary_ink())
                                .child(if let Some(issue) = &issue {
                                    if pending_edit {
                                        format!("GitHub issue #{} · edit unconfirmed", issue.number)
                                    } else {
                                        format!("GitHub issue #{}", issue.number)
                                    }
                                } else if uncertain {
                                    format!(
                                        "GitHub creation unconfirmed · check {}",
                                        item.pending_repo
                                            .as_deref()
                                            .unwrap_or("the original repository")
                                    )
                                } else {
                                    "Local project to-do".into()
                                }),
                        )
                        .when_some(issue.clone(), |d, issue| {
                            d.child(
                                labeled_secondary("todo-open-issue", "Open on GitHub", false, false)
                                    .flex_shrink_0()
                                    .on_click(move |_, _, cx| cx.open_url(&issue.url)),
                            )
                        }),
                )
                .child(
                    div()
                        .id("todo-details-scroll")
                        .debug_selector(|| "todo-details-scroll".into())
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_3()
                        .py_3()
                        .child(
                            div()
                                .w_full()
                                .max_w(px(760.))
                                .mx_auto()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(page::label().child("Title"))
                                .child(page::field_frame().w_full().child(
                                    div().flex_1().min_w_0().child(editor.title.clone()),
                                ))
                                .child(page::label().child("Description"))
                                .child(
                                    page::field_frame()
                                        .h_auto()
                                        .min_h(px(140.))
                                        .items_start()
                                        .py_2()
                                        .child(div().flex_1().min_w_0().child(editor.description.clone())),
                                )
                                .child(page::label().child("Attachments"))
                .children(
                    attachment_refs
                        .iter()
                        .enumerate()
                        .map(|(index, reference)| {
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(page::META_SIZE))
                                        .child(reference.clone()),
                                )
                                .child(
                                    page::icon_action(
                                        SharedString::from(format!(
                                            "todo-attachment-remove-{index}"
                                        )),
                                        "Remove attachment",
                                        true,
                                    )
                                    .child(icon("close", page::ICON_SIZE))
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| this.remove_attachment(index, cx),
                                    )),
                                )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            labeled_secondary("todo-add-files", "Add files…", false, false)
                                .debug_selector(|| "todo-add-files".into())
                                .flex_shrink_0()
                                .on_click(cx.listener(|this, _, _, cx| this.add_attachment_files(cx))),
                        )
                        .child(
                            page::field_frame()
                                .flex_grow(1.)
                                .flex_shrink(1.)
                                .flex_basis(px(180.))
                                .min_w(px(150.))
                                .max_w(px(320.))
                                .child(div().flex_1().min_w_0().child(editor.url.clone())),
                        )
                        .child(
                            labeled_secondary("todo-add-url", "Add link", false, false)
                                .debug_selector(|| "todo-add-url".into())
                                .flex_shrink_0()
                                .on_click(cx.listener(|this, _, _, cx| this.add_attachment_url(cx))),
                        ),
                )
                .when(remote && !item.attachments.is_empty(), |d| {
                    d.child(div().flex().flex_col().gap_1().children(
                        item.attachments.iter().enumerate().map(|(index, url)| {
                            let url = url.clone();
                            labeled_secondary(
                                SharedString::from(format!("todo-existing-attachment-{index}")),
                                format!("Open GitHub attachment {}", index + 1),
                                false,
                                false,
                            )
                            .on_click(move |_, _, cx| cx.open_url(&url))
                        }),
                    ))
                })
                .child(div().text_size(px(page::META_SIZE)).child("Depends on"))
                .children(
                    selected_dependencies
                        .iter()
                        .enumerate()
                        .map(|(index, reference)| {
                            let reference = reference.clone();
                            div()
                                .id(SharedString::from(format!("todo-selected-dependency-{index}")))
                                .flex()
                                .items_start()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded(px(radius::CONTROL))
                                .bg(content(0.05))
                                .child(div().flex_1().min_w_0().text_size(px(page::LABEL_SIZE)).child(self.dependency_label(item, &reference)))
                                .when(!dependencies_truncated, |d| {
                                    d.child(page::icon_action(
                                        SharedString::from(format!("todo-remove-dependency-{index}")),
                                        "Remove dependency",
                                        true,
                                    )
                                    .child(icon("close", page::ICON_SIZE))
                                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_dependency(reference.clone(), cx))))
                                })
                        }),
                )
                .when(dependencies_truncated, |d| {
                    d.child(page::meta().child("Additional dependencies are on GitHub. Edit dependency links there."))
                })
                .when(!dependencies_truncated, |d| {
                    d.child(
                        labeled_secondary(
                            "todo-choose-dependency",
                            if self.show_dependency_choices {
                                "Hide task choices"
                            } else {
                                "Add dependency…"
                            },
                            false,
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_dependency_choices = !this.show_dependency_choices;
                            cx.notify();
                        })),
                    )
                })
                .when(self.show_dependency_choices && !dependencies_truncated, |d| {
                    d.child(
                        div()
                            .id("todo-dependency-choices")
                            .max_h(px(150.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(dependency_choices.iter().enumerate().map(
                                |(index, (reference, label, selected))| {
                                    let reference = reference.clone();
                                    labeled_secondary(
                                        SharedString::from(format!(
                                            "todo-dependency-choice-{index}"
                                        )),
                                        format!("{} {label}", if *selected { "✓" } else { "+" }),
                                        false,
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.toggle_dependency(reference.clone(), cx)
                                        },
                                    ))
                                },
                            ))
                            .when(dependency_choices.is_empty(), |d| {
                                d.child("No other tasks on this board")
                            }),
                    )
                })
                            ),
                )
                .child(
                    div()
                        .id("todo-details-footer")
                        .debug_selector(|| "todo-details-footer".into())
                        .flex_shrink_0()
                        .border_t_1()
                        .border_color(page::divider())
                        .px_3()
                        .py_2()
                        .child(
                            div()
                                .w_full()
                                .max_w(px(760.))
                                .mx_auto()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                        .child(
                            labeled_primary(
                                "todo-details-save",
                                "Save",
                                self.pending_changes > 0 || pending_edit || uncertain,
                            )
                            .debug_selector(|| "todo-details-save".into())
                            .on_click(cx.listener(|this, _, _, cx| this.save_details(cx))),
                        )
                        .when(!uncertain && !pending_edit && !editor.matches_revision(item), |d| {
                            let label = if self.saved_details_key(item).is_some() {
                                "Discard draft and open saved draft"
                            } else {
                                "Discard draft and reload"
                            };
                            d.child(
                                labeled_secondary("todo-reload-details", label, false, self.pending_changes > 0)
                                    .debug_selector(|| "todo-reload-details".into())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.pending_changes > 0 { return; }
                                        let Some(item) = this.open_detail_item().cloned() else { return; };
                                        this.details = None;
                                        this.change_error = None;
                                        this.open_details(&item, cx);
                                    })),
                            )
                        })
                        .when(can_publish, |d| {
                            d.child(
                                labeled_secondary(
                                    "todo-publish",
                                    "Publish to GitHub",
                                    false,
                                    self.pending_changes > 0,
                                )
                                .debug_selector(|| "todo-publish".into())
                                .on_click(cx.listener(|this, _, _, cx| this.publish_details(cx))),
                            )
                        })
                        .when(uncertain || pending_edit, |d| {
                            d.child(
                                labeled_secondary(
                                    "todo-reconcile",
                                    if pending_edit {
                                        "Check issue for edit"
                                    } else {
                                        "Check for created issue"
                                    },
                                    false,
                                    self.pending_changes > 0,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.reconcile_detail(cx))),
                            )
                        })
                        .when(uncertain || pending_edit, |d| {
                            d.child(
                                labeled_secondary(
                                    "todo-retry",
                                    if self.confirm_retry {
                                        "I checked GitHub: retry"
                                    } else {
                                        "Retry after checking GitHub"
                                    },
                                    false,
                                    self.pending_changes > 0,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.retry_detail(cx))),
                            )
                        })
                        .when(uncertain, |d| {
                            d.child(
                                labeled_secondary(
                                    "todo-cancel-publish",
                                    if self.confirm_cancel {
                                        "I checked: keep local"
                                    } else {
                                        "Keep as local task"
                                    },
                                    false,
                                    self.pending_changes > 0,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.cancel_publish_detail(cx)),
                                ),
                            )
                        })
                        .when(pending_edit, |d| {
                            d.child(
                                labeled_secondary(
                                    "todo-discard-edit",
                                    if self.confirm_discard {
                                        "I checked: discard draft"
                                    } else {
                                        "Discard saved edit"
                                    },
                                    true,
                                    self.pending_changes > 0,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.discard_detail(cx))),
                            )
                        }),
                        ),
                )
                .into_any_element(),
        )
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
            TodoStatus::InReview => icon("eye", 15.)
                .text_color(accent_color())
                .into_any_element(),
            TodoStatus::Done => icon("check-circle", 15.)
                .text_color(vorkspace_ui::theme::status(StatusColor::Success, 1.))
                .into_any_element(),
        }
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
            .gap_2()
            .min_w_0()
            .min_h(px(64.))
            .rounded(px(radius::SURFACE))
            .drag_over::<DraggedTodo>(|s, _, _, _| s.bg(content(0.04)))
            .on_drop(cx.listener(move |this, dragged: &DraggedTodo, _, cx| {
                this.move_to(dragged, status, None, cx)
            }))
            .child(
                div()
                    .h(px(page::CONTROL_HEIGHT))
                    .px_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(page::META_SIZE))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(page::secondary_ink())
                    .child(Self::status_mark(status))
                    .child(status.label())
                    .child(div().child(count.to_string())),
            )
            .children(items.iter().map(|item| self.row(item, cx)))
            .when(count == 0, |d| {
                d.child(
                    div()
                        .px_1()
                        .py_1()
                        .text_size(px(12.))
                        .text_color(page::secondary_ink())
                        .child(match status {
                            TodoStatus::Todo => "Nothing to do",
                            TodoStatus::InProgress => "Nothing in progress",
                            TodoStatus::InReview => "Nothing in review",
                            TodoStatus::Done => "Nothing done yet",
                        }),
                )
            })
    }
}

impl Render for TodoBoardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.create_open && self.draft.focus_handle(cx).is_focused(window) {
            window.focus(&self.create_focus, cx);
        }
        if self.details.is_none()
            && let Some(focus) = self.detail_return_focus.take()
            && (window.focused(cx).is_none() || self.detail_focus.contains_focused(window, cx))
        {
            window.focus(&focus, cx);
        }
        // Flex wrapping follows this pane's actual width, including when it
        // is tiled inside a much wider window. Sidebar boards stay stacked.
        let padding = px(8.);
        let body = div()
            .id("todo-board-columns")
            .debug_selector(|| "todo-board-columns".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(padding)
            .pt(padding)
            // Let the last cards scroll clear of the floating create button.
            .pb(px(68.))
            .map(|d| {
                if self.columns {
                    d.flex().flex_wrap().items_start().gap_3()
                } else {
                    d.flex().flex_col().gap_2()
                }
            })
            .children(TodoStatus::ALL.into_iter().map(|status| {
                self.column(status, cx).when(self.columns, |d| {
                    d.flex_grow(1.)
                        .flex_shrink(0.)
                        .flex_basis(px(176.))
                        .min_w(px(176.))
                })
            }));
        let root = div()
            .id("todo-board")
            .debug_selector(|| "todo-board".into())
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col();
        let error = self
            .change_error
            .clone()
            .or(self.error.clone())
            .map(|error| {
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(gpui::rgb(0xe5484d))
                    .child(error)
            });
        root.children(error)
            .children(self.github_notice.clone().map(|notice| {
                div()
                    .id("todo-github-notice")
                    .px_3()
                    .pb_2()
                    .text_size(px(page::META_SIZE))
                    .text_color(page::secondary_ink())
                    .child(notice)
            }))
            .child(body)
            .child(self.create_button(cx))
            .children(self.create_popover(cx))
            .children(self.details_overlay(window, cx))
            .into_any_element()
    }
}

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
pub(crate) struct TodosPage {
    store: Database,
    board: Entity<TodoBoardView>,
    choices: Vec<BoardChoice>,
    open_counts: std::collections::HashMap<String, usize>,
    legacy: Vec<LegacyTasks>,
    assign_error: Option<String>,
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
            legacy: Vec::new(),
            assign_error: None,
            anchor: AnchorSlot::default(),
            picker_open: false,
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
        cx.spawn(async move |this, cx| {
            if let Ok((counts, legacy)) = work.await {
                let _ = this.update(cx, |this, cx| {
                    this.open_counts = counts;
                    this.legacy = legacy;
                    cx.notify();
                });
            }
        })
        .detach();
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
                        .map(|error| div().text_color(gpui::rgb(0xe5484d)).child(error)),
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod screenshots;
