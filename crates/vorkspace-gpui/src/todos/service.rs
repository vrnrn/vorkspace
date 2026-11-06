//! Project to-do orchestration. SQLite is locked only for local transitions;
//! GitHub reads and writes run on the caller's background executor without
//! holding the shared database lock.
use serde_json::{Value, json};
use std::path::Path;
use vorkspace_core::todos::{TodoBoard, TodoIssue, TodoItem, TodoStatus, TodoSync, github};
use vorkspace_runtime::runtime::{Database, locked};

// The native app owns these operations for UI and CLI callers. Keep control
// actions out of an in-flight mutation; receipts independently protect durable
// state against late completions and survive a process restart.
struct RemoteOperation(String);
static REMOTE_OPERATIONS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

impl RemoteOperation {
    fn acquire(key: String) -> Result<Self, String> {
        let mut active = REMOTE_OPERATIONS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !active.insert(key.clone()) {
            return Err("A GitHub operation for this task is still running; wait before changing or reconciling it".into());
        }
        Ok(Self(key))
    }

    fn issue(repo: &str, number: i64) -> Result<Self, String> {
        Self::acquire(format!("github:{}#{number}", repo.to_ascii_lowercase()))
    }

    fn publication(board: &TodoBoard, id: &str) -> Result<Self, String> {
        Self::acquire(format!("publish:{}:{id}", board.key()))
    }
}

impl Drop for RemoteOperation {
    fn drop(&mut self) {
        REMOTE_OPERATIONS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.0);
    }
}

pub(super) struct BoardSnapshot {
    pub items: Vec<TodoItem>,
    pub github_error: Option<String>,
    pub github_notice: Option<String>,
    pub github_repo: Option<String>,
}

pub(super) fn list(store: &Database, board: &TodoBoard) -> Result<BoardSnapshot, String> {
    let local = locked(store)?.list_todos(board)?;
    let pending_edits = locked(store)?.pending_remote_todo_edits(board)?;
    let TodoBoard::Project(project) = board else {
        return Ok(BoardSnapshot {
            items: local,
            github_error: None,
            github_notice: None,
            github_repo: None,
        });
    };
    let repo = match github::repo(Path::new(project)) {
        Ok(Some(repo)) => repo,
        Ok(None) => {
            return Ok(BoardSnapshot {
                items: attach_pending(local, pending_edits),
                github_error: None,
                github_notice: None,
                github_repo: None,
            });
        }
        Err(error) => {
            return Ok(BoardSnapshot {
                items: attach_pending(local, pending_edits),
                github_error: Some(error),
                github_notice: None,
                github_repo: None,
            });
        }
    };
    let remote = match github::list(Path::new(project), &repo) {
        Ok(remote) => remote,
        Err(error) => {
            return Ok(BoardSnapshot {
                items: attach_pending(local, pending_edits),
                github_error: Some(error),
                github_notice: None,
                github_repo: Some(repo),
            });
        }
    };
    let github_notice = remote
        .truncated
        .then(|| "Only the latest 500 GitHub issues are shown.".into());
    let remote = remote.items;
    let mut items = merge_remote(local, remote);
    items = attach_pending(items, pending_edits);
    items.sort_by_key(|item| {
        (
            vorkspace_core::todos::TodoStatus::ALL
                .iter()
                .position(|status| *status == item.status)
                .unwrap_or(0),
            item.position,
        )
    });
    Ok(BoardSnapshot {
        items,
        github_error: None,
        github_notice,
        github_repo: Some(repo),
    })
}

/// A pending publication keeps its local receipt identity until explicit
/// reconciliation settles it. A matching issue must not hide that recovery path.
fn merge_remote(local: Vec<TodoItem>, mut remote: Vec<TodoItem>) -> Vec<TodoItem> {
    let mut items = Vec::new();
    for item in local {
        if matches!(item.sync, TodoSync::Pending | TodoSync::Uncertain) {
            if let Some(token) = &item.submission_token {
                remote.retain(|other| other.submission_token.as_ref() != Some(token));
            }
            items.push(item);
        } else if !item.issue.as_ref().is_some_and(|issue| {
            remote.iter().any(|other| {
                other.issue.as_ref().is_some_and(|candidate| {
                    candidate.repo.eq_ignore_ascii_case(&issue.repo)
                        && candidate.number == issue.number
                })
            })
        }) {
            items.push(item);
        }
    }
    items.extend(remote);
    items
}

fn attach_pending(mut items: Vec<TodoItem>, pending: Vec<TodoItem>) -> Vec<TodoItem> {
    for edit in pending {
        if let Some(existing) = items.iter_mut().find(|item| {
            item.id == edit.id
                || item
                    .issue
                    .as_ref()
                    .zip(edit.issue.as_ref())
                    .is_some_and(|(a, b)| {
                        a.repo.eq_ignore_ascii_case(&b.repo) && a.number == b.number
                    })
        }) {
            if existing.id == edit.id {
                existing.pending_edit = edit.pending_edit;
            } else {
                *existing = edit;
            }
        } else {
            items.push(edit);
        }
    }
    items
}

fn saved_edit_applied(current: &TodoItem, before: &TodoItem, input: &Value) -> bool {
    if input.get("attachments").is_some() {
        return false;
    }
    if input
        .get("title")
        .and_then(Value::as_str)
        .is_some_and(|title| title.trim() != current.title)
    {
        return false;
    }
    if input
        .get("description")
        .and_then(Value::as_str)
        .is_some_and(|description| !github::description_edit_applied(current, before, description))
    {
        return false;
    }
    if input
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status != current.status.id())
    {
        return false;
    }
    if let Some(values) = input.get("dependencies").and_then(Value::as_array) {
        let Some(issue) = current
            .issue
            .as_ref()
            .filter(|issue| !issue.dependencies_truncated)
        else {
            return false;
        };
        let Some(requested) = values
            .iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        match (
            github::dependency_refs(&requested, &issue.repo),
            github::dependency_refs(&current.dependencies, &issue.repo),
        ) {
            (Ok(requested), Ok(actual)) if requested == actual => {}
            _ => return false,
        }
    }
    true
}

fn remote_snapshot(repo: &str, number: i64, input: &Value) -> TodoItem {
    let url = format!("https://github.com/{repo}/issues/{number}");
    TodoItem {
        id: format!("github:{repo}#{number}"),
        title: input
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("GitHub issue (offline)")
            .into(),
        description: String::new(),
        attachments: vec![],
        dependencies: vec![],
        status: TodoStatus::Todo,
        position: 0,
        created_at: 0,
        updated_at: 0,
        issue: Some(TodoIssue {
            repo: repo.into(),
            number,
            url,
            labels: vec![],
            dependencies_truncated: false,
        }),
        sync: TodoSync::Synced,
        pending_edit: None,
        submission_token: None,
        pending_repo: None,
    }
}

fn input_id(input: &Value) -> Result<&str, String> {
    input
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or("Provide id".into())
}

fn github_repo(board: &TodoBoard) -> Result<(&Path, String), String> {
    let TodoBoard::Project(project) = board else {
        return Err("Choose a project board".into());
    };
    let root = Path::new(project);
    let repo = github::repo(root)?
        .ok_or("This project has no GitHub origin; local to-dos remain available")?;
    Ok((root, repo))
}

fn publish(
    store: &Database,
    board: &TodoBoard,
    id: &str,
    expected_revision: Option<i64>,
) -> Result<Value, String> {
    let _operation = RemoteOperation::publication(board, id)?;
    publish_admitted(store, board, id, false, expected_revision)
}

fn publish_admitted(
    store: &Database,
    board: &TodoBoard,
    id: &str,
    retry: bool,
    expected_revision: Option<i64>,
) -> Result<Value, String> {
    let (root, repo) = github_repo(board)?;
    let preflight = locked(store)?
        .get_todo(board, id)?
        .ok_or("To-do not found")?;
    if expected_revision.is_some_and(|revision| revision != preflight.updated_at) {
        return Err(
            "This to-do changed while preparing publication; reload its details before publishing"
                .into(),
        );
    }
    github::validate_create(root, &repo, &preflight)?;
    let (draft, token) = if retry {
        if !preflight
            .pending_repo
            .as_deref()
            .is_some_and(|submitted| submitted.eq_ignore_ascii_case(&repo))
        {
            return Err(
                "The project's GitHub origin changed since this issue was submitted".into(),
            );
        }
        locked(store)?.retry_todo_publish(board, id, preflight.updated_at)?
    } else {
        locked(store)?.begin_todo_publish(board, id, &repo, preflight.updated_at)?
    };
    let result = github::create(
        root,
        &repo,
        &draft.title,
        &draft.description,
        &draft.attachments,
        &draft.dependencies,
        &token,
    );
    let remote = match result {
        Ok(remote) => remote,
        Err(error) => {
            let _ = locked(store)?.uncertain_todo_publish(board, id);
            return Err(format!(
                "GitHub creation is unconfirmed for local to-do {id}; it was not retried. Reconcile before trying again: {error}"
            ));
        }
    };
    let issue = remote
        .issue
        .as_ref()
        .ok_or("GitHub did not return issue identity")?;
    let _issue_operation = RemoteOperation::issue(&issue.repo, issue.number)?;
    let follow_up = locked(store)?.finish_todo_publish(board, id, &remote)?;
    if let Some(patch) = follow_up {
        match github::update(root, &repo, issue.number, &patch.input) {
            Ok(updated) => {
                locked(store)?.clear_remote_todo_edit(board, &remote.id, &patch.attempt_id)?;
                return Ok(json!(updated));
            }
            Err(error) => {
                let _ =
                    locked(store)?.uncertain_remote_todo_edit(board, &remote.id, &patch.attempt_id);
                return Err(format!(
                    "Issue {} was created, but its requested status is unconfirmed and saved for reconciliation: {error}",
                    issue.url
                ));
            }
        }
    }
    Ok(json!(remote))
}

/// App/board actions share one explicit owner. `github:true` on add or
/// `todos.publish` is the only path that submits a local item to GitHub.
pub(crate) fn action(
    store: &Database,
    board: &TodoBoard,
    action: &str,
    input: &Value,
) -> Result<Value, String> {
    match action {
        "todos.list" => {
            let snapshot = list(store, board)?;
            Ok(
                json!({"board":board.key(),"items":snapshot.items,"githubError":snapshot.github_error,"githubNotice":snapshot.github_notice}),
            )
        }
        "todos.add" => {
            let mut local_input = input.clone();
            if let Some(object) = local_input.as_object_mut() {
                object.remove("github");
            }
            let added = locked(store)?.todo_action(board, action, &local_input)?;
            if input
                .get("github")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                publish(
                    store,
                    board,
                    added["id"].as_str().ok_or("New to-do has no ID")?,
                    None,
                )
            } else {
                Ok(added)
            }
        }
        "todos.publish" => {
            let revision = input
                .get("expectedUpdatedAt")
                .map(|value| value.as_i64().ok_or("expectedUpdatedAt must be an integer"))
                .transpose()?;
            publish(store, board, input_id(input)?, revision)
        }
        "todos.reconcile" | "todos.retry" | "todos.cancelPublish" => {
            let id = input_id(input)?;
            let _operation = RemoteOperation::publication(board, id)?;
            let draft = locked(store)?
                .get_todo(board, id)?
                .ok_or("To-do not found")?;
            if !matches!(draft.sync, TodoSync::Pending | TodoSync::Uncertain) {
                return Err("To-do has no uncertain GitHub submission".into());
            }
            let submitted_repo = draft
                .pending_repo
                .as_deref()
                .ok_or("The submitted GitHub repository is missing")?;
            let token = locked(store)?
                .todo_submission_token(board, id)?
                .ok_or("Submission token is missing")?;
            let current_repo = github_repo(board).ok();
            let pinned = current_repo
                .as_ref()
                .filter(|(_, repo)| repo.eq_ignore_ascii_case(submitted_repo));
            if let Some((root, repo)) = pinned {
                match github::find_submission(root, repo, &token) {
                    Ok(Some(found)) => {
                        let issue = found
                            .issue
                            .as_ref()
                            .ok_or("Reconciled issue has no identity")?;
                        let _issue_operation = RemoteOperation::issue(&issue.repo, issue.number)?;
                        let follow_up = locked(store)?.finish_todo_publish(board, id, &found)?;
                        if let Some(edit) = &follow_up {
                            // Reconciliation only reads GitHub. No status write
                            // is in flight, so an explicit retry can run now.
                            locked(store)?.uncertain_remote_todo_edit(
                                board,
                                &found.id,
                                &edit.attempt_id,
                            )?;
                        }
                        return Ok(
                            json!({"found":true,"item":found,"pendingEdit":follow_up.is_some()}),
                        );
                    }
                    Ok(None) => {}
                    Err(error) if action != "todos.cancelPublish" => return Err(error),
                    Err(_) => {}
                }
            } else if action != "todos.cancelPublish" {
                return Err(format!(
                    "The project no longer points to the submitted repository {submitted_repo}; restore that origin before reconciling or retrying"
                ));
            }
            if action == "todos.retry" || action == "todos.cancelPublish" {
                if input.get("confirmNoIssue").and_then(Value::as_bool) != Some(true) {
                    return Err(format!(
                        "Check {submitted_repo} on GitHub and confirm that no issue was created"
                    ));
                }
                if action == "todos.cancelPublish" {
                    return Ok(json!(locked(store)?.cancel_todo_publish(board, id)?));
                }
                return publish_admitted(store, board, id, true, Some(draft.updated_at));
            }
            Ok(json!({"found":false,"uncertain":true,"id":id}))
        }
        "todos.reconcileEdit" | "todos.retryEdit" | "todos.discardEdit" => {
            let id = input_id(input)?;
            let saved = locked(store)?
                .pending_remote_todo_edits(board)?
                .into_iter()
                .find(|item| item.id == id)
                .ok_or("No pending edit for this issue")?;
            let issue = saved
                .issue
                .as_ref()
                .ok_or("Pending issue identity is unavailable")?;
            let _operation = RemoteOperation::issue(&issue.repo, issue.number)?;
            let edit = saved
                .pending_edit
                .as_ref()
                .ok_or("Pending edit is unavailable")?;
            if action == "todos.discardEdit" {
                if input.get("confirmChecked").and_then(Value::as_bool) != Some(true) {
                    return Err(
                        "Check the issue on GitHub before discarding this local edit".into(),
                    );
                }
                locked(store)?.clear_remote_todo_edit(board, id, &edit.attempt_id)?;
                return Ok(json!({"discarded":true}));
            }
            let (root, repo) = github_repo(board)?;
            let number = github::parse_issue_id(id, &repo)
                .ok_or("Choose a GitHub issue from this project")?;
            let current = github::view(root, &repo, number)?;
            if saved_edit_applied(&current, &saved, &edit.input) {
                locked(store)?.clear_remote_todo_edit(board, id, &edit.attempt_id)?;
                return Ok(json!({"applied":true,"item":current}));
            }
            if action == "todos.reconcileEdit" {
                return Ok(json!({"applied":false,"item":current,"pendingEdit":edit}));
            }
            if input.get("confirmNoEffect").and_then(Value::as_bool) != Some(true) {
                return Err("Check the issue on GitHub before retrying this edit".into());
            }
            github::validate_update(root, &repo, &edit.input)?;
            let attempt = locked(store)?.retry_remote_todo_edit(board, id, &edit.attempt_id)?;
            let mut patch = attempt.input;
            // An explicit retry applies the saved fields to the version the
            // caller just checked, while retaining the final read guard.
            patch["expectedUpdatedAt"] = json!(current.updated_at);
            match github::update(root, &repo, number, &patch) {
                Ok(updated) => {
                    locked(store)?.clear_remote_todo_edit(board, id, &attempt.attempt_id)?;
                    Ok(json!(updated))
                }
                Err(error) => {
                    let _ =
                        locked(store)?.uncertain_remote_todo_edit(board, id, &attempt.attempt_id);
                    Err(format!(
                        "GitHub edit is unconfirmed; it was not retried: {error}"
                    ))
                }
            }
        }
        "todos.update" | "todos.remove" => {
            let id = input_id(input)?;
            let local = locked(store)?;
            let issue = local.get_todo(board, id)?.and_then(|item| item.issue);
            if issue.is_none() && !id.starts_with("github:") {
                return local.todo_action(board, action, input);
            }
            drop(local);
            let (root, repo) = github_repo(board).map_err(|_| {
                if issue.is_some() {
                    "The GitHub origin is unavailable; the linked issue was not changed"
                } else {
                    "Issue is outside this project"
                }
            })?;
            let number = if let Some(issue) = issue {
                if !issue.repo.eq_ignore_ascii_case(&repo) {
                    return Err("Linked issue belongs to another repository".into());
                }
                issue.number
            } else {
                github::parse_issue_id(id, &repo).ok_or("Issue is outside this project")?
            };
            if input.get("before").is_some() {
                return Err("GitHub issue ordering cannot be changed".into());
            }
            let update = if action == "todos.remove" {
                json!({"status":"done"})
            } else {
                input.clone()
            };
            let _operation = RemoteOperation::issue(&repo, number)?;
            github::validate_update(root, &repo, &update)?;
            let snapshot = match github::view(root, &repo, number) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let snapshot = remote_snapshot(&repo, number, &update);
                    let attempt =
                        locked(store)?.begin_remote_todo_edit(board, &snapshot, &update)?;
                    locked(store)?.uncertain_remote_todo_edit(board, &snapshot.id, &attempt)?;
                    return Err(format!(
                        "GitHub is unavailable; the requested edit was saved locally and was not sent: {error}"
                    ));
                }
            };
            if input
                .get("expectedUpdatedAt")
                .and_then(Value::as_i64)
                .is_some_and(|expected| expected != snapshot.updated_at)
            {
                return Err("This GitHub issue changed after it was opened; reopen its details before saving".into());
            }
            let attempt = locked(store)?.begin_remote_todo_edit(board, &snapshot, &update)?;
            let result = match github::update(root, &repo, number, &update) {
                Ok(result) => {
                    locked(store)?.clear_remote_todo_edit(board, &snapshot.id, &attempt)?;
                    result
                }
                Err(error) => {
                    let _ =
                        locked(store)?.uncertain_remote_todo_edit(board, &snapshot.id, &attempt);
                    return Err(format!(
                        "GitHub edit is unconfirmed; the requested change is saved locally and was not replayed: {error}"
                    ));
                }
            };
            if action == "todos.remove" {
                Ok(json!({"closed":true,"item":result}))
            } else {
                Ok(json!(result))
            }
        }
        _ => Err("Unknown to-do action".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext;
    use vorkspace_core::store::Store;

    #[gpui::test]
    fn published_dependency_stays_openable_online_offline_and_after_reopen(
        cx: &mut gpui::TestAppContext,
    ) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("tasks.sqlite");
        let board = TodoBoard::Project("/dependency-test".into());
        let store = Store::open(&path).unwrap();
        let dependency = store
            .add_todo(&board, "Dependency", TodoStatus::Todo)
            .unwrap();
        let task = store
            .add_todo_with_details(
                &board,
                "Dependent task",
                TodoStatus::Todo,
                "",
                &[],
                std::slice::from_ref(&dependency.id),
            )
            .unwrap();
        let view = cx.new(|cx| {
            super::super::TodoBoardView::new(
                std::sync::Arc::new(std::sync::Mutex::new(
                    Store::open(Path::new(":memory:")).unwrap(),
                )),
                false,
                cx,
            )
        });
        let check = |items: Vec<TodoItem>, expected: &str, cx: &mut gpui::TestAppContext| {
            view.update(cx, |view, _| {
                view.items = items;
                let task = view.items.iter().find(|item| item.id == task.id).unwrap();
                let linked = view
                    .dependency_item(task, &task.dependencies[0])
                    .expect("dependency is openable");
                assert_eq!(linked.id, expected);
                assert_eq!(linked.title, "Dependency");
            });
        };
        check(store.list_todos(&board).unwrap(), &dependency.id, cx);
        store
            .begin_todo_publish(
                &board,
                &dependency.id,
                "test/project",
                dependency.updated_at,
            )
            .unwrap();
        let remote = remote_snapshot("test/project", 12, &json!({"title":"Dependency"}));
        store
            .finish_todo_publish(&board, &dependency.id, &remote)
            .unwrap();
        let saved = store.get_todo(&board, &task.id).unwrap().unwrap();
        assert_eq!(
            saved.dependencies,
            [remote.issue.as_ref().unwrap().url.clone()]
        );
        assert!(
            saved.updated_at > task.updated_at,
            "the changed link advances the task revision"
        );
        check(
            merge_remote(store.list_todos(&board).unwrap(), vec![remote.clone()]),
            &remote.id,
            cx,
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        // Revision-checked editors must reload after the link changes. A
        // caller supplying an older local ID still resolves the new identity.
        store.todo_action(&board, "todos.update", &json!({
            "id":task.id, "title":"Edited after publication", "dependencies":[dependency.id],
        })).unwrap();
        assert_eq!(
            store
                .get_todo(&board, &task.id)
                .unwrap()
                .unwrap()
                .dependencies,
            [remote.issue.as_ref().unwrap().url.clone()]
        );
        check(store.list_todos(&board).unwrap(), &dependency.id, cx);
        check(
            merge_remote(store.list_todos(&board).unwrap(), vec![remote.clone()]),
            &remote.id,
            cx,
        );
    }

    #[test]
    fn saved_description_edit_requires_retaining_attachment_links() {
        let mut before = remote_snapshot("example/project", 7, &json!({"title":"Task"}));
        before.description = "Old details".into();
        before.attachments = vec!["https://example.com/screenshot".into()];
        let requested = json!({"description":"New details"});
        let mut current = before.clone();
        current.description = "New details".into();
        current.attachments.clear();
        assert!(!saved_edit_applied(&current, &before, &requested));
        current.description =
            "New details\n\n[Attachment](<https://example.com/screenshot>)".into();
        current.attachments = before.attachments.clone();
        assert!(saved_edit_applied(&current, &before, &requested));
    }

    #[gpui::test]
    fn dependency_selection_keeps_same_number_issues_distinct_after_origin_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::super::TodoBoardView;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tasks.sqlite");
        let board = TodoBoard::Project(directory.path().to_string_lossy().into_owned());
        let database: Database =
            std::sync::Arc::new(std::sync::Mutex::new(Store::open(&path).unwrap()));
        let old = remote_snapshot("Old/App", 7, &json!({"title":"Dependency"}));
        let current = remote_snapshot("New/App", 7, &json!({"title":"Dependency"}));
        let (task, receipt, items) = {
            let store = locked(&database).unwrap();
            let receipt = store
                .add_todo(&board, "Dependency", TodoStatus::Todo)
                .unwrap();
            store
                .begin_todo_publish(&board, &receipt.id, "Old/App", receipt.updated_at)
                .unwrap();
            store
                .finish_todo_publish(&board, &receipt.id, &old)
                .unwrap();
            let task = store
                .add_todo_with_details(
                    &board,
                    "Dependent",
                    TodoStatus::Todo,
                    "",
                    &[],
                    &["#7".into()],
                )
                .unwrap();
            let items = merge_remote(store.list_todos(&board).unwrap(), vec![current.clone()]);
            (task, receipt.id, items)
        };
        let view = cx.new(|cx| TodoBoardView::new(database, false, cx));
        let old_ref = "https://github.com/old/app/issues/7";
        let new_ref = "https://github.com/new/app/issues/7";
        view.update(cx, |view, cx| {
            view.board = Some(board.clone());
            view.github_repo = Some("New/App".into());
            view.items = items;
            view.open_details(&task, cx);
            assert_eq!(TodoBoardView::dependency_ref(&old), old_ref);
            assert_eq!(TodoBoardView::dependency_ref(&current), new_ref);
            assert_eq!(
                view.dependency_label(&task, old_ref),
                "Old/App#7 · Dependency"
            );
            assert_eq!(
                view.dependency_label(&task, new_ref),
                "New/App#7 · Dependency"
            );
            assert_eq!(view.dependency_item(&task, "#7").unwrap().id, current.id);
            let old_owner = remote_snapshot("Old/App", 12, &json!({"title":"Former issue"}));
            assert_eq!(view.dependency_item(&old_owner, "#7").unwrap().id, receipt);
            view.toggle_dependency(TodoBoardView::dependency_ref(&old), cx);
            view.toggle_dependency(TodoBoardView::dependency_ref(&current), cx);
            let editor = view.details.as_ref().unwrap();
            assert_eq!(
                TodoBoardView::dependency_values(&editor.dependencies, cx),
                [old_ref]
            );
            view.toggle_dependency(TodoBoardView::dependency_ref(&current), cx);
            view.save_details(cx);
        });
        cx.run_until_parked();
        let store = Store::open(&path).unwrap();
        let saved = store.get_todo(&board, &task.id).unwrap().unwrap();
        assert_eq!(saved.dependencies, [old_ref, new_ref]);
        assert_eq!(
            github::dependency_refs(&saved.dependencies, "New/App").unwrap(),
            ["#7", old_ref],
        );
        view.update(cx, |view, cx| {
            view.items = merge_remote(store.list_todos(&board).unwrap(), vec![current.clone()]);
            view.open_details(&saved, cx);
            assert_eq!(
                view.dependency_item(&saved, "https://github.com/OLD/APP/issues/7")
                    .unwrap()
                    .id,
                receipt
            );
            assert_eq!(
                view.dependency_item(&saved, new_ref).unwrap().id,
                current.id
            );
            let editor = view.details.as_ref().unwrap();
            assert_eq!(
                TodoBoardView::dependency_values(&editor.dependencies, cx),
                [old_ref, new_ref]
            );
        });
    }

    #[test]
    fn matched_create_keeps_reconciliation_and_initial_status() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let board = TodoBoard::Project("/project".into());
        let draft = store
            .add_todo(&board, "Review this", TodoStatus::InReview)
            .unwrap();
        let (pending, token) = store
            .begin_todo_publish(&board, &draft.id, "example/project", draft.updated_at)
            .unwrap();
        let mut remote = pending.clone();
        remote.id = "github:example/project#42".into();
        remote.status = TodoStatus::Todo;
        remote.sync = TodoSync::Synced;
        remote.submission_token = Some(token.clone());
        remote.issue = Some(TodoIssue {
            repo: "example/project".into(),
            number: 42,
            url: "https://github.com/example/project/issues/42".into(),
            labels: vec![],
            dependencies_truncated: false,
        });
        for state in [TodoSync::Pending, TodoSync::Uncertain] {
            if state == TodoSync::Uncertain {
                store.uncertain_todo_publish(&board, &draft.id).unwrap();
            }
            let items = merge_remote(store.list_todos(&board).unwrap(), vec![remote.clone()]);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].id, draft.id);
            assert_eq!(items[0].sync, state);
            assert_eq!(items[0].status, TodoStatus::InReview);
            assert_eq!(items[0].submission_token.as_deref(), Some(token.as_str()));
        }
        let followup = store
            .finish_todo_publish(&board, &draft.id, &remote)
            .unwrap()
            .unwrap();
        assert_eq!(followup.input["status"], "inReview");
        let edits = store.pending_remote_todo_edits(&board).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].id, remote.id);
        let items = merge_remote(store.list_todos(&board).unwrap(), vec![remote.clone()]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, remote.id);
    }

    #[test]
    fn active_edit_blocks_discard_and_late_completion_cannot_clear_a_successor() {
        let store: Database = std::sync::Arc::new(std::sync::Mutex::new(
            Store::open(Path::new(":memory:")).unwrap(),
        ));
        let board = TodoBoard::Project("/receipt-owner-test".into());
        let snapshot = remote_snapshot("test/receipt-owner", 7, &json!({"title":"Task"}));
        let first = locked(&store)
            .unwrap()
            .begin_remote_todo_edit(&board, &snapshot, &json!({"title":"First"}))
            .unwrap();
        let operation = RemoteOperation::issue("test/receipt-owner", 7).unwrap();
        let discard = json!({"id":snapshot.id,"confirmChecked":true});
        assert!(
            action(&store, &board, "todos.discardEdit", &discard)
                .unwrap_err()
                .contains("still running")
        );
        assert!(RemoteOperation::issue("TEST/receipt-owner", 7).is_err());
        drop(operation);
        action(&store, &board, "todos.discardEdit", &discard).unwrap();
        let successor = locked(&store)
            .unwrap()
            .begin_remote_todo_edit(&board, &snapshot, &json!({"title":"Successor"}))
            .unwrap();
        assert!(
            locked(&store)
                .unwrap()
                .clear_remote_todo_edit(&board, &snapshot.id, &first)
                .is_err()
        );
        assert!(
            locked(&store)
                .unwrap()
                .uncertain_remote_todo_edit(&board, &snapshot.id, &first)
                .is_err()
        );
        let saved = locked(&store)
            .unwrap()
            .pending_remote_todo_edits(&board)
            .unwrap();
        let edit = saved[0].pending_edit.as_ref().unwrap();
        assert_eq!(edit.attempt_id, successor);
        assert_eq!(edit.input["title"], "Successor");
        assert_eq!(edit.phase, TodoSync::Pending);
    }
}
