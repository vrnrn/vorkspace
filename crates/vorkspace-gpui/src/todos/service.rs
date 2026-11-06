//! Project to-do orchestration. SQLite is locked only for local transitions;
//! GitHub reads and writes run on the caller's background executor without
//! holding the shared database lock.
use serde_json::{Value, json};
use std::path::Path;
use vorkspace_core::todos::{TodoBoard, TodoIssue, TodoItem, TodoStatus, TodoSync, github};
use vorkspace_runtime::runtime::{Database, locked};

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
    // A matching private marker de-duplicates a pending local draft in this
    // read-only view. Only an explicit reconcile action settles its receipt.
    let mut items = local
        .into_iter()
        .filter(|item| {
            // Remote data is canonical for a linked row and for a just-reconciled
            // pending row. Keep local snapshots when a remote issue is absent.
            !item.issue.as_ref().is_some_and(|issue| {
                remote.iter().any(|other| {
                    other.issue.as_ref().is_some_and(|candidate| {
                        candidate.repo == issue.repo && candidate.number == issue.number
                    })
                })
            }) && !item.submission_token.as_ref().is_some_and(|token| {
                remote
                    .iter()
                    .any(|other| other.submission_token.as_ref() == Some(token))
            })
        })
        .collect::<Vec<_>>();
    items.extend(remote);
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

fn attach_pending(mut items: Vec<TodoItem>, pending: Vec<TodoItem>) -> Vec<TodoItem> {
    for edit in pending {
        if let Some(existing) = items.iter_mut().find(|item| {
            item.id == edit.id
                || item
                    .issue
                    .as_ref()
                    .zip(edit.issue.as_ref())
                    .is_some_and(|(a, b)| a.repo == b.repo && a.number == b.number)
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

fn saved_edit_applied(current: &TodoItem, input: &Value) -> bool {
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
        .is_some_and(|description| description.trim() != current.description)
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

fn publish(store: &Database, board: &TodoBoard, id: &str, retry: bool) -> Result<Value, String> {
    let (root, repo) = github_repo(board)?;
    let preflight = locked(store)?
        .get_todo(board, id)?
        .ok_or("To-do not found")?;
    github::validate_create(root, &repo, &preflight)?;
    let (draft, token) = if retry {
        if preflight.pending_repo.as_deref() != Some(repo.as_str()) {
            return Err(
                "The project's GitHub origin changed since this issue was submitted".into(),
            );
        }
        locked(store)?.retry_todo_publish(board, id)?
    } else {
        locked(store)?.begin_todo_publish(board, id, &repo)?
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
    let follow_up = locked(store)?.finish_todo_publish(board, id, &remote)?;
    if let Some(patch) = follow_up {
        match github::update(root, &repo, issue.number, &patch) {
            Ok(updated) => {
                locked(store)?.clear_remote_todo_edit(board, &remote.id)?;
                return Ok(json!(updated));
            }
            Err(error) => {
                let _ = locked(store)?.uncertain_remote_todo_edit(board, &remote.id);
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
                    false,
                )
            } else {
                Ok(added)
            }
        }
        "todos.publish" => publish(store, board, input_id(input)?, false),
        "todos.reconcile" | "todos.retry" | "todos.cancelPublish" => {
            let id = input_id(input)?;
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
                .filter(|(_, repo)| repo == submitted_repo);
            if let Some((root, repo)) = pinned {
                match github::find_submission(root, repo, &token) {
                    Ok(Some(found)) => {
                        let follow_up = locked(store)?.finish_todo_publish(board, id, &found)?;
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
                return publish(store, board, id, true);
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
            let edit = saved.pending_edit.ok_or("Pending edit is unavailable")?;
            if action == "todos.discardEdit" {
                if input.get("confirmChecked").and_then(Value::as_bool) != Some(true) {
                    return Err(
                        "Check the issue on GitHub before discarding this local edit".into(),
                    );
                }
                locked(store)?.clear_remote_todo_edit(board, id)?;
                return Ok(json!({"discarded":true}));
            }
            let (root, repo) = github_repo(board)?;
            let number = github::parse_issue_id(id, &repo)
                .ok_or("Choose a GitHub issue from this project")?;
            let current = github::view(root, &repo, number)?;
            if saved_edit_applied(&current, &edit.input) {
                locked(store)?.clear_remote_todo_edit(board, id)?;
                return Ok(json!({"applied":true,"item":current}));
            }
            if action == "todos.reconcileEdit" {
                return Ok(json!({"applied":false,"item":current,"pendingEdit":edit}));
            }
            if input.get("confirmNoEffect").and_then(Value::as_bool) != Some(true) {
                return Err("Check the issue on GitHub before retrying this edit".into());
            }
            let mut patch = locked(store)?.retry_remote_todo_edit(board, id)?;
            // An explicit retry applies the saved fields to the version the
            // caller just checked, while retaining the final read guard.
            patch["expectedUpdatedAt"] = json!(current.updated_at);
            match github::update(root, &repo, number, &patch) {
                Ok(updated) => {
                    locked(store)?.clear_remote_todo_edit(board, id)?;
                    Ok(json!(updated))
                }
                Err(error) => {
                    let _ = locked(store)?.uncertain_remote_todo_edit(board, id);
                    Err(format!(
                        "GitHub edit is unconfirmed; it was not retried: {error}"
                    ))
                }
            }
        }
        "todos.update" | "todos.remove" => {
            let id = input_id(input)?;
            let local = locked(store)?.get_todo(board, id)?;
            let (root, repo) = match board {
                TodoBoard::Project(_) => github_repo(board)
                    .map(|(root, repo)| (Some(root), Some(repo)))
                    .unwrap_or((None, None)),
                _ => (None, None),
            };
            let issue = local
                .as_ref()
                .and_then(|item| item.issue.as_ref())
                .map(|issue| (issue.repo.clone(), issue.number));
            let number = repo
                .as_ref()
                .and_then(|repo| github::parse_issue_id(id, repo))
                .or_else(|| issue.as_ref().map(|(_, number)| *number));
            if id.starts_with("github:") && number.is_none() {
                return Err("Issue is outside this project".into());
            }
            if let Some(number) = number {
                let root = root
                    .ok_or("The GitHub origin is unavailable; the linked issue was not changed")?;
                let repo = repo
                    .ok_or("The GitHub origin is unavailable; the linked issue was not changed")?;
                if issue
                    .as_ref()
                    .is_some_and(|(linked_repo, _)| linked_repo != &repo)
                {
                    return Err("Linked issue belongs to another repository".into());
                }
                if input.get("before").is_some() {
                    return Err("GitHub issue ordering cannot be changed".into());
                }
                let update = if action == "todos.remove" {
                    json!({"status":"done"})
                } else {
                    input.clone()
                };
                let snapshot = match github::view(root, &repo, number) {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        let snapshot = remote_snapshot(&repo, number, &update);
                        locked(store)?.begin_remote_todo_edit(board, &snapshot, &update)?;
                        locked(store)?.uncertain_remote_todo_edit(board, &snapshot.id)?;
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
                locked(store)?.begin_remote_todo_edit(board, &snapshot, &update)?;
                let result = match github::update(root, &repo, number, &update) {
                    Ok(result) => {
                        locked(store)?.clear_remote_todo_edit(board, &snapshot.id)?;
                        result
                    }
                    Err(error) => {
                        let _ = locked(store)?.uncertain_remote_todo_edit(board, &snapshot.id);
                        return Err(format!(
                            "GitHub edit is unconfirmed; the requested change is saved locally and was not replayed: {error}"
                        ));
                    }
                };
                return if action == "todos.remove" {
                    Ok(json!({"closed":true,"item":result}))
                } else {
                    Ok(json!(result))
                };
            }
            if local
                .as_ref()
                .is_some_and(|item| item.sync != TodoSync::Local)
            {
                return Err(
                    "This GitHub submission is pending or uncertain; reconcile it before editing"
                        .into(),
                );
            }
            locked(store)?.todo_action(board, action, input)
        }
        _ => Err("Unknown to-do action".into()),
    }
}
