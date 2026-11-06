//! GitHub issue backed project boards. The Inbox's bounded, authenticated `gh`
//! runner owns subprocess policy; this module only builds validated issue calls.
use super::{clean_description, clean_title, TodoIssue, TodoItem, TodoStatus, TodoSync};
use crate::inbox::{run_gh, run_git, timestamp_ms};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const IN_PROGRESS_LABEL: &str = "vorkspace:in-progress";
pub const IN_REVIEW_LABEL: &str = "vorkspace:in-review";
const ISSUE_FIELDS: &str = "number,title,url,state,body,labels,blockedBy,createdAt,updatedAt";
const ISSUE_LIMIT: usize = 500;

pub struct IssuePage {
    pub items: Vec<TodoItem>,
    pub truncated: bool,
}

/// Determine the repository from the project's pinned origin without an
/// authenticated network request. An absent or non-GitHub origin is local.
pub fn repo(root: &Path) -> Result<Option<String>, String> {
    if run_git(root, &["rev-parse", "--show-toplevel"]).is_err() {
        return Ok(None);
    }
    let origin = match run_git(root, &["config", "--get", "remote.origin.url"]) {
        Ok(origin) => origin,
        Err(_) => return Ok(None),
    };
    parse_origin(&origin)
}

fn parse_origin(raw: &str) -> Result<Option<String>, String> {
    let raw = raw.trim();
    let path = if let Some(path) = raw.strip_prefix("git@github.com:") {
        path.to_string()
    } else if let Ok(url) = url::Url::parse(raw) {
        if url.host_str() != Some("github.com") {
            return Ok(None);
        }
        if !matches!(url.scheme(), "https" | "ssh")
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port().is_some()
            || (url.scheme() == "ssh" && url.username() != "git")
            || (url.scheme() == "https" && !url.username().is_empty())
        {
            return Err("Unsupported GitHub origin".into());
        }
        url.path().trim_start_matches('/').to_string()
    } else {
        return Ok(None);
    };
    let path = path
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_end_matches('/'));
    let Some((owner, name)) = path.split_once('/') else {
        return Err("Invalid GitHub origin".into());
    };
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || !owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("Invalid GitHub origin".into());
    }
    Ok(Some(format!("{owner}/{name}")))
}

fn pinned(root: &Path, expected: &str) -> Result<(), String> {
    if repo(root)?.as_deref() != Some(expected) {
        return Err("Project GitHub origin changed; reopen the board before writing".into());
    }
    Ok(())
}

fn json_items(raw: &str, repo: &str) -> Result<Vec<TodoItem>, String> {
    let rows: Vec<Value> = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    rows.iter()
        .enumerate()
        .map(|(position, row)| issue(row, repo, position as i64))
        .collect()
}

fn issue(row: &Value, repo: &str, position: i64) -> Result<TodoItem, String> {
    let number = row
        .get("number")
        .and_then(Value::as_i64)
        .filter(|n| *n > 0)
        .ok_or("GitHub issue has no valid number")?;
    let title = row
        .get("title")
        .and_then(Value::as_str)
        .ok_or("GitHub issue has no title")?
        .to_string();
    let url = row
        .get("url")
        .and_then(Value::as_str)
        .ok_or("GitHub issue has no URL")?
        .to_string();
    if issue_number_from_url(&url, repo) != Some(number) {
        return Err("GitHub issue URL does not match this project".into());
    }
    let body = row.get("body").and_then(Value::as_str).unwrap_or("");
    let description = body
        .lines()
        .filter(|line| !line.trim().starts_with("<!-- vorkspace-todo:"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    let labels = row
        .get("labels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let label_names = labels
        .iter()
        .filter_map(|label| label.get("name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let has_label = |name: &str| {
        label_names
            .iter()
            .any(|value| value.eq_ignore_ascii_case(name))
    };
    let status = if row
        .get("state")
        .and_then(Value::as_str)
        .is_some_and(|state| state.eq_ignore_ascii_case("closed"))
    {
        TodoStatus::Done
    } else if has_label(IN_REVIEW_LABEL) {
        TodoStatus::InReview
    } else if has_label(IN_PROGRESS_LABEL) {
        TodoStatus::InProgress
    } else {
        TodoStatus::Todo
    };
    let dependency_nodes = row
        .get("blockedBy")
        .and_then(|value| value.as_array().or_else(|| value.get("nodes")?.as_array()));
    let dependencies = dependency_nodes
        .into_iter()
        .flatten()
        .map(|value| {
            let url = value
                .get("url")
                .and_then(Value::as_str)
                .ok_or("GitHub dependency has no issue URL")?;
            dependency_ref(url, repo)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let dependencies_truncated = row
        .get("blockedBy")
        .and_then(|value| value.get("totalCount"))
        .and_then(Value::as_u64)
        .is_some_and(|total| total > dependencies.len() as u64);
    Ok(TodoItem {
        id: format!("github:{repo}#{number}"),
        title,
        description,
        attachments: attachment_urls(body),
        dependencies,
        status,
        position,
        created_at: timestamp_ms(row.get("createdAt").and_then(Value::as_str).unwrap_or("")),
        updated_at: timestamp_ms(row.get("updatedAt").and_then(Value::as_str).unwrap_or("")),
        issue: Some(TodoIssue {
            repo: repo.into(),
            number,
            url,
            labels: label_names,
            dependencies_truncated,
        }),
        sync: TodoSync::Synced,
        pending_edit: None,
        submission_token: body
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("<!-- vorkspace-todo:")?
                    .strip_suffix(" -->")
                    .map(str::to_owned)
            })
            .next_back(),
        pending_repo: None,
    })
}

fn attachment_urls(body: &str) -> Vec<String> {
    let pattern =
        regex::Regex::new(r"!?\[([^\]]*)\]\(<?(https://[^)\s>]+)>?\)").expect("valid regex");
    let mut urls = Vec::new();
    for captures in pattern.captures_iter(body) {
        let label = &captures[1];
        let url = &captures[2];
        if let Ok(url) = url::Url::parse(url) {
            if (label == "Attachment"
                || matches!(
                    url.host_str(),
                    Some(
                        "github.com"
                            | "user-images.githubusercontent.com"
                            | "githubusercontent.com"
                    )
                ))
                && !urls.iter().any(|other| other == url.as_str())
            {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

pub fn list(root: &Path, expected_repo: &str) -> Result<IssuePage, String> {
    pinned(root, expected_repo)?;
    let raw = run_gh(
        root,
        &[
            "issue",
            "list",
            "--state",
            "all",
            "--limit",
            &ISSUE_LIMIT.to_string(),
            "--repo",
            expected_repo,
            "--json",
            ISSUE_FIELDS,
        ],
        false,
    )?;
    let items = json_items(&raw, expected_repo)?;
    Ok(IssuePage {
        truncated: items.len() >= ISSUE_LIMIT,
        items,
    })
}

pub fn view(root: &Path, expected_repo: &str, number: i64) -> Result<TodoItem, String> {
    pinned(root, expected_repo)?;
    if number <= 0 {
        return Err("Invalid issue number".into());
    }
    let number = number.to_string();
    let raw = run_gh(
        root,
        &[
            "issue",
            "view",
            &number,
            "--repo",
            expected_repo,
            "--json",
            ISSUE_FIELDS,
        ],
        false,
    )?;
    let row: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    issue(&row, expected_repo, 0)
}

pub fn parse_issue_id(id: &str, expected_repo: &str) -> Option<i64> {
    id.strip_prefix(&format!("github:{expected_repo}#"))?
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
}

fn dependency_ref(value: &str, repo: &str) -> Result<String, String> {
    let value = value.trim();
    if let Some((target_repo, number)) = issue_ref_from_url(value) {
        return Ok(if target_repo.eq_ignore_ascii_case(repo) {
            format!("#{number}")
        } else {
            format!(
                "https://github.com/{}/issues/{number}",
                target_repo.to_ascii_lowercase()
            )
        });
    }
    let number = parse_issue_id(value, repo)
        .or_else(|| value.strip_prefix('#').unwrap_or(value).parse::<i64>().ok())
        .filter(|number| *number > 0)
        .ok_or("GitHub dependencies must be positive issue numbers or GitHub issue URLs")?;
    Ok(format!("#{number}"))
}

pub fn dependency_refs(values: &[String], repo: &str) -> Result<Vec<String>, String> {
    if values.len() > super::MAX_DEPENDENCIES {
        return Err("Too many dependencies".into());
    }
    let mut refs = Vec::new();
    for value in values {
        let reference = dependency_ref(value, repo)?;
        if !refs.contains(&reference) {
            refs.push(reference);
        }
    }
    refs.sort();
    Ok(refs)
}

fn attachment_inputs(
    root: &Path,
    values: &[String],
) -> Result<(Vec<PathBuf>, Vec<String>), String> {
    if values.len() > super::MAX_ATTACHMENTS {
        return Err("Too many attachments".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    let mut urls = Vec::new();
    for value in values {
        if value.is_empty()
            || value.len() > 4096
            || value.chars().any(|ch| matches!(ch, '\n' | '\r' | '\0'))
        {
            return Err("Invalid attachment reference".into());
        }
        if value.starts_with("https://") {
            let url = url::Url::parse(value).map_err(|_| "Invalid attachment URL")?;
            if url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || value.chars().any(char::is_whitespace)
            {
                return Err("Invalid attachment URL".into());
            }
            if !urls.iter().any(|existing| existing == url.as_str()) {
                urls.push(url.to_string());
            }
        } else {
            let path = Path::new(value);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            };
            let path = path
                .canonicalize()
                .map_err(|e| format!("Attachment is unavailable: {e}"))?;
            if !path.starts_with(&root) || !path.is_file() {
                return Err("Attachments must be files inside this project".into());
            }
            paths.push(path);
        }
    }
    Ok((paths, urls))
}

fn body_with_attachments(description: &str, urls: &[String], token: Option<&str>) -> String {
    let mut body = description.trim().to_string();
    for url in urls {
        if !body.contains(url) {
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str(&format!("[Attachment](<{url}>)"));
        }
    }
    if let Some(token) = token {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(&format!("<!-- vorkspace-todo:{token} -->"));
    }
    body
}

fn issue_url(raw: &str, repo: &str) -> Result<i64, String> {
    raw.lines()
        .find_map(|line| issue_number_from_url(line.trim(), repo))
        .ok_or("GitHub did not return the created issue URL".into())
}

fn issue_number_from_url(raw: &str, repo: &str) -> Option<i64> {
    let (target_repo, number) = issue_ref_from_url(raw)?;
    target_repo.eq_ignore_ascii_case(repo).then_some(number)
}

pub(crate) fn issue_ref_from_url(raw: &str) -> Option<(String, i64)> {
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut segments = url.path_segments()?;
    let owner = segments.next()?;
    let name = segments.next()?;
    let repo = parse_origin(&format!("https://github.com/{owner}/{name}")).ok()??;
    if segments.next()? != "issues" {
        return None;
    }
    let number = segments.next()?.parse::<i64>().ok().filter(|n| *n > 0)?;
    if segments.next().is_some() {
        return None;
    }
    Some((repo, number))
}

/// The caller first persists a pending row. A failure here is uncertain: gh
/// may have created the issue (including partial attachment uploads).
pub fn create(
    root: &Path,
    expected_repo: &str,
    title: &str,
    description: &str,
    attachments: &[String],
    dependencies: &[String],
    token: &str,
) -> Result<TodoItem, String> {
    pinned(root, expected_repo)?;
    let title = clean_title(title)?;
    let description = clean_description(description)?;
    let (paths, urls) = attachment_inputs(root, attachments)?;
    let dependencies = dependency_refs(dependencies, expected_repo)?;
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("Invalid submission token".into());
    }
    let body = body_with_attachments(&description, &urls, Some(token));
    let mut args = vec![
        "issue".to_string(),
        "create".into(),
        "--repo".into(),
        expected_repo.into(),
        "--title".into(),
        title,
        "--body".into(),
        body,
    ];
    for path in paths {
        args.extend(["--attach".into(), path.to_string_lossy().into_owned()]);
    }
    if !dependencies.is_empty() {
        args.extend([
            "--blocked-by".into(),
            dependencies
                .iter()
                .map(|value| value.trim_start_matches('#'))
                .collect::<Vec<_>>()
                .join(","),
        ]);
    }
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    pinned(root, expected_repo)?;
    let output = run_gh(root, &refs, false)?;
    let number = issue_url(&output, expected_repo)?;
    view(root, expected_repo, number)
}

pub fn validate_create(root: &Path, expected_repo: &str, item: &TodoItem) -> Result<(), String> {
    pinned(root, expected_repo)?;
    clean_title(&item.title)?;
    clean_description(&item.description)?;
    let (files, _) = attachment_inputs(root, &item.attachments)?;
    dependency_refs(&item.dependencies, expected_repo)?;
    let mut flags = Vec::new();
    if !files.is_empty() {
        flags.push("--attach");
    }
    if !item.dependencies.is_empty() {
        flags.push("--blocked-by");
    }
    check_cli_options(root, "create", &flags)?;
    Ok(())
}

/// Preflight capabilities before recording an attempted external write. Older
/// GitHub CLIs must leave the local draft editable rather than make it uncertain.
pub fn validate_update(root: &Path, expected_repo: &str, input: &Value) -> Result<(), String> {
    pinned(root, expected_repo)?;
    let attachments = input
        .get("attachments")
        .map(|value| {
            value
                .as_array()
                .ok_or("attachments must be an array")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or("attachments must contain strings")
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let (files, _) = attachment_inputs(root, &attachments)?;
    let mut flags = Vec::new();
    if !files.is_empty() {
        flags.push("--attach");
    }
    if input.get("dependencies").is_some() {
        flags.extend(["--add-blocked-by", "--remove-blocked-by"]);
    }
    check_cli_options(root, "edit", &flags)
}

fn check_cli_options(root: &Path, command: &str, flags: &[&str]) -> Result<(), String> {
    if flags.is_empty() {
        return Ok(());
    }
    let help = run_gh(root, &["issue", command, "--help"], false)?;
    require_cli_options(&help, flags)
}

fn require_cli_options(help: &str, flags: &[&str]) -> Result<(), String> {
    for flag in flags {
        if !help.lines().any(|line| {
            line.trim_start().starts_with(flag)
                && line.trim_start()[flag.len()..].starts_with(char::is_whitespace)
        }) {
            return Err(format!("Update GitHub CLI to use {flag}, or remove that attachment/dependency. No GitHub change was sent."));
        }
    }
    Ok(())
}

fn ensure_label(root: &Path, repo: &str, label: &str) -> Result<String, String> {
    let listed = || -> Result<Option<String>, String> {
        let raw = run_gh(
            root,
            &[
                "label", "list", "--repo", repo, "--limit", "1000", "--json", "name",
            ],
            false,
        )?;
        let labels: Vec<Value> = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        Ok(labels
            .iter()
            .filter_map(|value| value.get("name").and_then(Value::as_str))
            .find(|name| name.eq_ignore_ascii_case(label))
            .map(str::to_owned))
    };
    if let Some(name) = listed()? {
        return Ok(name);
    }
    pinned(root, repo)?;
    let result = run_gh(
        root,
        &[
            "label",
            "create",
            label,
            "--repo",
            repo,
            "--color",
            "3b82f6",
            "--description",
            "Vorkspace to-do status",
        ],
        true,
    );
    match result {
        Ok(_) => Ok(label.into()),
        Err(error) => listed()?.ok_or(error),
    }
}

fn append_status_label_removals(args: &mut Vec<String>, existing: &[String], target: Option<&str>) {
    for label in existing {
        let reserved = label.eq_ignore_ascii_case(IN_PROGRESS_LABEL)
            || label.eq_ignore_ascii_case(IN_REVIEW_LABEL);
        if reserved && target.is_none_or(|target| !label.eq_ignore_ascii_case(target)) {
            args.extend(["--remove-label".into(), label.clone()]);
        }
    }
}

/// Edits only requested fields. Dependencies use GitHub's native blocked-by
/// links; state labels are reserved to this app and other labels are retained.
pub fn update(
    root: &Path,
    expected_repo: &str,
    number: i64,
    input: &Value,
) -> Result<TodoItem, String> {
    pinned(root, expected_repo)?;
    let current = view(root, expected_repo, number)?;
    if input
        .get("expectedUpdatedAt")
        .and_then(Value::as_i64)
        .is_some_and(|expected| expected != current.updated_at)
    {
        return Err(
            "This GitHub issue changed after it was opened; reopen its details before saving"
                .into(),
        );
    }
    if input.get("dependencies").is_some()
        && current
            .issue
            .as_ref()
            .is_some_and(|issue| issue.dependencies_truncated)
    {
        return Err("The full dependency list is unavailable; edit dependencies on GitHub".into());
    }
    let title = input
        .get("title")
        .and_then(Value::as_str)
        .map(clean_title)
        .transpose()?;
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .map(clean_description)
        .transpose()?;
    let attachments = input
        .get("attachments")
        .map(|value| {
            value
                .as_array()
                .ok_or("attachments must be an array of file paths")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or("attachments must be an array of file paths")
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let (paths, urls) = attachment_inputs(root, &attachments)?;
    let dependencies = input
        .get("dependencies")
        .map(|value| {
            value
                .as_array()
                .ok_or("dependencies must be an array")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or("dependencies must be an array of issue numbers")
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let desired = dependencies
        .as_ref()
        .map(|values| dependency_refs(values, expected_repo))
        .transpose()?;
    let existing = dependency_refs(&current.dependencies, expected_repo)?;
    let mut args = vec![
        "issue".to_string(),
        "edit".into(),
        number.to_string(),
        "--repo".into(),
        expected_repo.into(),
    ];
    if let Some(title) = title {
        args.extend(["--title".into(), title]);
    }
    if description.is_some() || !urls.is_empty() {
        let body = body_with_attachments(
            description.as_deref().unwrap_or(&current.description),
            &urls,
            current.submission_token.as_deref(),
        );
        args.extend(["--body".into(), body]);
    }
    for path in paths {
        args.extend(["--attach".into(), path.to_string_lossy().into_owned()]);
    }
    if let Some(desired) = desired {
        for value in desired.iter().filter(|value| !existing.contains(value)) {
            args.extend([
                "--add-blocked-by".into(),
                value.trim_start_matches('#').into(),
            ]);
        }
        for value in existing.iter().filter(|value| !desired.contains(value)) {
            args.extend([
                "--remove-blocked-by".into(),
                value.trim_start_matches('#').into(),
            ]);
        }
    }
    let status = input
        .get("status")
        .and_then(Value::as_str)
        .map(|id| TodoStatus::from_id(id).ok_or("Invalid to-do status"))
        .transpose()?;
    if let Some(status) = status.filter(|status| *status != TodoStatus::Done) {
        let target = match status {
            TodoStatus::InProgress => Some(IN_PROGRESS_LABEL),
            TodoStatus::InReview => Some(IN_REVIEW_LABEL),
            _ => None,
        };
        let existing_labels = &current
            .issue
            .as_ref()
            .ok_or("GitHub issue metadata is missing")?
            .labels;
        if let Some(target) = target.filter(|target| {
            !existing_labels
                .iter()
                .any(|name| name.eq_ignore_ascii_case(target))
        }) {
            let label = ensure_label(root, expected_repo, target)?;
            args.extend(["--add-label".into(), label]);
        }
        append_status_label_removals(&mut args, existing_labels, target);
    }
    if args.len() > 6 {
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        pinned(root, expected_repo)?;
        run_gh(root, &refs, true)?;
    }
    if let Some(status) = status {
        if status == TodoStatus::Done && current.status != TodoStatus::Done {
            pinned(root, expected_repo)?;
            run_gh(
                root,
                &[
                    "issue",
                    "close",
                    &number.to_string(),
                    "--repo",
                    expected_repo,
                    "--reason",
                    "completed",
                ],
                true,
            )?;
        } else if status != TodoStatus::Done && current.status == TodoStatus::Done {
            pinned(root, expected_repo)?;
            run_gh(
                root,
                &[
                    "issue",
                    "reopen",
                    &number.to_string(),
                    "--repo",
                    expected_repo,
                ],
                true,
            )?;
        }
    }
    view(root, expected_repo, number)
}

pub fn token_in_body(body: &str, token: &str) -> bool {
    body.lines()
        .any(|line| line.trim() == format!("<!-- vorkspace-todo:{token} -->"))
}

/// A bounded read-only reconciliation. Absence is not proof that an in-flight
/// GitHub create failed, so callers must never auto-resubmit on `None`.
pub fn find_submission(
    root: &Path,
    expected_repo: &str,
    token: &str,
) -> Result<Option<TodoItem>, String> {
    pinned(root, expected_repo)?;
    let raw = run_gh(
        root,
        &[
            "issue",
            "list",
            "--state",
            "all",
            "--limit",
            "500",
            "--repo",
            expected_repo,
            "--json",
            "number,body",
        ],
        false,
    )?;
    let rows: Vec<Value> = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    match submission_number(&rows, token)? {
        Some(number) => view(root, expected_repo, number).map(Some),
        None => Ok(None),
    }
}

fn submission_number(rows: &[Value], token: &str) -> Result<Option<i64>, String> {
    let matched = rows
        .iter()
        .find(|row| {
            row.get("body")
                .and_then(Value::as_str)
                .is_some_and(|body| token_in_body(body, token))
        })
        .map(|row| {
            row.get("number")
                .and_then(Value::as_i64)
                .ok_or_else(|| "Matched issue has no number".to_string())
                .and_then(|number| {
                    if number > 0 {
                        Ok(number)
                    } else {
                        Err("Matched issue has no valid number".into())
                    }
                })
        })
        .transpose()?;
    if matched.is_none() && rows.len() >= ISSUE_LIMIT {
        return Err("Cannot reconcile absence against a partial issue list".into());
    }
    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unsupported_cli_flags_fail_before_publication() {
        let old_help = "FLAGS\n  -b, --body string  Issue body\n      --attach-preview  Unrelated flag\nExamples mention --attach in prose";
        assert!(require_cli_options(old_help, &["--attach"]).is_err());
        assert!(require_cli_options(old_help, &[]).is_ok());
        let current_help = "FLAGS\n      --attach stringArray  Upload files\n      --add-blocked-by stringArray  Add dependencies\n      --remove-blocked-by stringArray  Remove dependencies\n";
        assert!(require_cli_options(
            current_help,
            &["--attach", "--add-blocked-by", "--remove-blocked-by"]
        )
        .is_ok());
        assert!(require_cli_options(current_help, &["--blocked-by"]).is_err());
    }

    #[test]
    fn origins_are_strict_and_repo_scoped() {
        assert_eq!(
            parse_origin("git@github.com:acme/app.git").unwrap(),
            Some("acme/app".into())
        );
        assert_eq!(
            parse_origin("https://github.com/acme/app.git").unwrap(),
            Some("acme/app".into())
        );
        assert_eq!(
            parse_origin("https://gitlab.com/acme/app.git").unwrap(),
            None
        );
        assert!(parse_origin("https://user:secret@github.com/acme/app").is_err());
        assert!(parse_origin("git@github.com:acme/app/extra").is_err());
    }
    #[test]
    fn existing_issues_map_to_four_states_and_native_links() {
        let row = json!({"number": 7,"title":"Ship","url":"https://github.com/acme/app/issues/7","state":"OPEN","body":"See ![screen](https://github.com/user-attachments/assets/a)","labels":[{"name":IN_REVIEW_LABEL}],"blockedBy":{"nodes":[{"number":5,"url":"https://github.com/acme/app/issues/5"}],"totalCount":1},"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z"});
        let item = issue(&row, "acme/app", 0).unwrap();
        assert_eq!(item.status, TodoStatus::InReview);
        assert_eq!(item.dependencies, ["#5"]);
        assert_eq!(item.attachments.len(), 1);
        assert!(!item.issue.as_ref().unwrap().dependencies_truncated);
        let mut closed = row;
        closed["state"] = json!("CLOSED");
        assert_eq!(
            issue(&closed, "acme/app", 0).unwrap().status,
            TodoStatus::Done
        );
    }

    #[test]
    fn moving_to_todo_removes_status_labels_with_their_existing_spelling() {
        let row = json!({
            "number":7,"title":"Ship","url":"https://github.com/acme/app/issues/7","state":"OPEN",
            "labels":[{"name":"Vorkspace:In-Progress"},{"name":"VORKSPACE:In-Review"},{"name":"Needs triage"}],
        });
        let item = issue(&row, "acme/app", 0).unwrap();
        assert_eq!(item.status, TodoStatus::InReview);
        let labels = item.issue.unwrap().labels;
        let mut args = Vec::new();
        append_status_label_removals(&mut args, &labels, None);
        assert_eq!(
            args,
            [
                "--remove-label",
                "Vorkspace:In-Progress",
                "--remove-label",
                "VORKSPACE:In-Review"
            ],
        );
        args.clear();
        append_status_label_removals(&mut args, &labels, Some(IN_PROGRESS_LABEL));
        assert_eq!(args, ["--remove-label", "VORKSPACE:In-Review"]);
    }

    #[test]
    fn dependency_identity_survives_same_number_in_another_repository() {
        let row = json!({
            "number":7,"title":"Ship","url":"https://github.com/acme/app/issues/7",
            "blockedBy":{"nodes":[
                {"number":5,"url":"https://github.com/acme/app/issues/5"},
                {"number":5,"url":"https://github.com/acme/upstream/issues/5"}
            ],"totalCount":3}
        });
        let item = issue(&row, "acme/app", 0).unwrap();
        assert_eq!(
            item.dependencies,
            ["#5", "https://github.com/acme/upstream/issues/5"]
        );
        assert!(item.issue.unwrap().dependencies_truncated);
        assert_eq!(
            dependency_refs(
                &[
                    "5".into(),
                    "#5".into(),
                    "github:acme/app#5".into(),
                    "https://github.com/ACME/APP/issues/5".into(),
                    "https://github.com/ACME/Upstream/issues/5".into(),
                ],
                "acme/app"
            )
            .unwrap(),
            item.dependencies
        );
        for invalid in [
            "#0",
            "https://example.com/acme/app/issues/5",
            "https://github.com/acme/app/issues/5?x=1",
            "https://user@github.com/acme/app/issues/5",
        ] {
            assert!(dependency_refs(&[invalid.into()], "acme/app").is_err());
        }
    }

    #[test]
    fn reconciliation_accepts_a_positive_match_in_a_bounded_list() {
        let mut rows = (1..=ISSUE_LIMIT)
            .map(|number| json!({"number":number,"body":"other"}))
            .collect::<Vec<_>>();
        assert!(submission_number(&rows, "receipt").is_err());
        rows[10]["body"] = json!("<!-- vorkspace-todo:receipt -->");
        assert_eq!(submission_number(&rows, "receipt").unwrap(), Some(11));
        assert_eq!(submission_number(&[], "receipt").unwrap(), None);
    }

    #[test]
    fn attachment_inputs_stay_inside_project_and_https_links_survive_body_edits() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let inside_file = project.path().join("screen.png");
        std::fs::write(&inside_file, b"screen").unwrap();
        let outside_file = outside.path().join("outside.png");
        std::fs::write(&outside_file, b"outside").unwrap();
        let (files, links) = attachment_inputs(
            project.path(),
            &[
                inside_file.to_string_lossy().into_owned(),
                "https://example.com/design.png".into(),
            ],
        )
        .unwrap();
        assert_eq!(files, [inside_file.canonicalize().unwrap()]);
        assert_eq!(links, ["https://example.com/design.png"]);
        assert!(attachment_inputs(
            project.path(),
            &[outside_file.to_string_lossy().into_owned()]
        )
        .is_err());
        let body = body_with_attachments("Updated description", &links, Some("receipt-token"));
        assert!(body.contains("[Attachment](<https://example.com/design.png>)"));
        assert!(token_in_body(&body, "receipt-token"));
        assert_eq!(attachment_urls(&body), links);
    }
}
