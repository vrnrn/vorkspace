//! GitHub Inbox models and parsers shared with the original implementation.
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
const MAX_PR_DIFF_BYTES: usize = 2 * 1024 * 1024;
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubLabel {
    pub name: String,
    pub color: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAssignee {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubWorkItem {
    pub kind: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub state_reason: String,
    pub created_at: String,
    pub updated_at: String,
    pub labels: Vec<GitHubLabel>,
    pub assignees: Vec<GitHubAssignee>,
    pub draft: bool,
    pub repo: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubWorkItemDetails {
    pub body: String,
    pub author: String,
    pub author_avatar_url: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub review_decision: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubWorkItemComment {
    pub id: String,
    pub kind: String,
    pub author: String,
    pub author_avatar_url: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
    pub state: String,
    pub path: String,
    pub line: Option<i64>,
    pub resolved: bool,
    pub thread_id: String,
    pub replies: Vec<GitHubWorkItemComment>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubWorkItemCommit {
    pub oid: String,
    pub message_headline: String,
    pub author: String,
    pub committed_date: String,
    pub url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubWorkItemThread {
    pub comments: Vec<GitHubWorkItemComment>,
    pub commits: Vec<GitHubWorkItemCommit>,
    pub truncated: bool,
    pub review_decision: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPrFile {
    pub path: String,
    pub additions: i64,
    pub deletions: i64,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPrDiff {
    pub additions: i64,
    pub deletions: i64,
    pub files: Vec<GitHubPrFile>,
    pub patch: String,
    pub truncated: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPrCheck {
    pub name: String,
    pub workflow: String,
    pub state: String,
    pub url: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPrChecks {
    pub head_oid: String,
    pub checks: Vec<GitHubPrCheck>,
}

pub(super) fn git_github_repositories_for(root: &Path) -> Result<Vec<String>, String> {
    let json = gh_checked(root, &["repo", "view", "--json", "nameWithOwner,parent"])?;
    parse_github_repositories(&json)
}

fn parse_github_repositories(json: &str) -> Result<Vec<String>, String> {
    #[derive(Deserialize)]
    struct Owner {
        login: String,
    }
    #[derive(Deserialize)]
    struct Parent {
        name: String,
        owner: Owner,
    }
    #[derive(Deserialize)]
    struct View {
        #[serde(rename = "nameWithOwner")]
        name_with_owner: String,
        #[serde(default)]
        parent: Option<Parent>,
    }

    let view: View = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let (owner, name) = split_github_repo(&view.name_with_owner)?;
    let mut repos = vec![format!("{owner}/{name}")];
    if let Some(parent) = view.parent {
        let parent = format!("{}/{}", parent.owner.login, parent.name);
        let (owner, name) = split_github_repo(&parent)?;
        let parent = format!("{owner}/{name}");
        if !repos[0].eq_ignore_ascii_case(&parent) {
            repos.push(parent);
        }
    }
    Ok(repos)
}

pub(super) fn git_github_work_items_for(
    root: &Path,
    repo: &str,
    kind: &str,
    assigned_to_me: bool,
    state: &str,
    search: &str,
    limit: u32,
) -> Result<Vec<GitHubWorkItem>, String> {
    let kind = kind.trim();
    if kind != "issue" && kind != "pr" {
        return Err("Unknown GitHub task kind".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let state = if state.trim().eq_ignore_ascii_case("all") {
        "all"
    } else {
        "open"
    };
    let limit = limit.clamp(1, 100).to_string();
    let fields = if kind == "pr" {
        "number,title,url,state,createdAt,updatedAt,labels,assignees,isDraft"
    } else {
        "number,title,url,state,stateReason,createdAt,updatedAt,labels,assignees"
    };
    let mut args = vec![
        kind.to_string(),
        "list".into(),
        "--state".into(),
        state.into(),
        "--limit".into(),
        limit,
        "--repo".into(),
        repo.clone(),
        "--json".into(),
        fields.into(),
    ];
    if assigned_to_me {
        args.push("--assignee".into());
        args.push("@me".into());
    }
    let search = search.trim();
    if !search.is_empty() {
        args.push("--search".into());
        args.push(search.to_string());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let json = gh_checked(root, &refs)?;
    parse_github_work_items(&json, kind, &repo)
}

pub(super) fn git_github_work_item_for(
    root: &Path,
    repo: &str,
    kind: &str,
    number: i64,
) -> Result<GitHubWorkItem, String> {
    let kind = kind.trim();
    if kind != "issue" && kind != "pr" {
        return Err("Unknown GitHub task kind".into());
    }
    if number <= 0 {
        return Err("GitHub task number must be positive".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let number = number.to_string();
    let fields = if kind == "pr" {
        "number,title,url,state,createdAt,updatedAt,labels,assignees,isDraft"
    } else {
        "number,title,url,state,stateReason,createdAt,updatedAt,labels,assignees"
    };
    let json = gh_checked(
        root,
        &[kind, "view", &number, "--repo", &repo, "--json", fields],
    )?;
    parse_github_work_item(&json, kind, &repo)
}

fn github_pr_action_args(repo: &str, number: i64, action: &str) -> Result<Vec<String>, String> {
    if number <= 0 {
        return Err("GitHub pull request number must be positive".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let number = number.to_string();
    let args = match action.trim() {
        "merge" => vec!["pr", "merge", &number, "--repo", &repo, "--merge"],
        "squash" => vec!["pr", "merge", &number, "--repo", &repo, "--squash"],
        "rebase" => vec!["pr", "merge", &number, "--repo", &repo, "--rebase"],
        "draft" => vec!["pr", "ready", &number, "--repo", &repo, "--undo"],
        "ready" => vec!["pr", "ready", &number, "--repo", &repo],
        "close" => vec!["pr", "close", &number, "--repo", &repo],
        "reopen" => vec!["pr", "reopen", &number, "--repo", &repo],
        _ => return Err("Unknown GitHub pull request action".into()),
    };
    Ok(args.into_iter().map(str::to_string).collect())
}

pub(super) fn git_github_pr_action_for(
    root: &Path,
    repo: &str,
    number: i64,
    action: &str,
) -> Result<GitHubWorkItem, String> {
    let args = github_pr_action_args(repo, number, action)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    // Successful mutation commands do not always write to stdout. Their exit
    // status confirms the action ran; the follow-up view fetches the new state.
    gh_run(root, &refs, true)?;
    git_github_work_item_for(root, repo, "pr", number)
}

pub(super) fn git_github_work_item_details_for(
    root: &Path,
    repo: &str,
    kind: &str,
    number: i64,
) -> Result<GitHubWorkItemDetails, String> {
    let kind = kind.trim();
    if kind != "issue" && kind != "pr" {
        return Err("Unknown GitHub task kind".into());
    }
    if number <= 0 {
        return Err("Invalid GitHub item number".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let number = number.to_string();
    let fields = if kind == "pr" {
        "body,author,baseRefName,headRefName,reviewDecision"
    } else {
        "body,author"
    };
    let json = gh_checked(
        root,
        &[kind, "view", &number, "--repo", &repo, "--json", fields],
    )?;
    parse_github_work_item_details(&json)
}

fn parse_github_work_item_details(json: &str) -> Result<GitHubWorkItemDetails, String> {
    #[derive(Deserialize)]
    struct Author {
        #[serde(default)]
        login: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        #[serde(default)]
        body: String,
        #[serde(default)]
        author: Option<Author>,
        #[serde(default)]
        base_ref_name: String,
        #[serde(default)]
        head_ref_name: String,
        #[serde(default)]
        review_decision: Option<String>,
    }
    let row: Row = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let author = row.author.map(|author| author.login).unwrap_or_default();
    let author_avatar_url = github_avatar_url(&author);
    Ok(GitHubWorkItemDetails {
        body: row.body,
        author,
        author_avatar_url,
        base_ref_name: row.base_ref_name,
        head_ref_name: row.head_ref_name,
        review_decision: row.review_decision.unwrap_or_default(),
    })
}

const GITHUB_ISSUE_THREAD_QUERY: &str = r#"
query InboxIssueThread($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    issue(number: $number) {
      comments(last: 40) {
        totalCount
        nodes {
          id
          author { login }
          body
          createdAt
          url
          isMinimized
        }
      }
    }
  }
}
"#;

const GITHUB_PR_THREAD_QUERY: &str = r#"
query InboxPullRequestThread($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewDecision
      baseRefName
      headRefName
      commits(last: 40) {
        totalCount
        nodes {
          commit {
            oid
            messageHeadline
            committedDate
            url
            author {
              name
              user { login }
            }
          }
        }
      }
      comments(last: 40) {
        totalCount
        nodes {
          id
          author { login }
          body
          createdAt
          url
          isMinimized
        }
      }
      reviews(last: 40) {
        totalCount
        nodes {
          id
          author { login }
          body
          state
          submittedAt
          url
        }
      }
      reviewThreads(last: 20) {
        totalCount
        nodes {
          id
          isResolved
          path
          comments(first: 8) {
            totalCount
            nodes {
              id
              author { login }
              body
              createdAt
              url
              path
              line
              originalLine
              isMinimized
            }
          }
        }
      }
    }
  }
}
"#;

const GITHUB_REVIEW_REPLY_MUTATION: &str = r#"
mutation InboxReviewReply($threadId: ID!, $body: String!) {
  addPullRequestReviewThreadReply(input: {
    pullRequestReviewThreadId: $threadId
    body: $body
  }) {
    comment { url }
  }
}
"#;

pub(super) fn git_github_work_item_thread_for(
    root: &Path,
    repo: &str,
    kind: &str,
    number: i64,
) -> Result<GitHubWorkItemThread, String> {
    let kind = kind.trim();
    if kind != "issue" && kind != "pr" {
        return Err("Unknown GitHub task kind".into());
    }
    if number <= 0 {
        return Err("Invalid GitHub item number".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let query = if kind == "pr" {
        GITHUB_PR_THREAD_QUERY
    } else {
        GITHUB_ISSUE_THREAD_QUERY
    };
    let owner_field = format!("owner={owner}");
    let name_field = format!("name={name}");
    let number_field = format!("number={number}");
    let json = gh_checked(
        root,
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &owner_field,
            "-F",
            &name_field,
            "-F",
            &number_field,
        ],
    )?;
    parse_github_work_item_thread(&json, kind)
}

fn github_comment_input<'a>(
    kind: &'a str,
    number: i64,
    body: &'a str,
) -> Result<(&'a str, &'a str), String> {
    let kind = kind.trim();
    if kind != "issue" && kind != "pr" {
        return Err("Unknown GitHub task kind".into());
    }
    if number <= 0 {
        return Err("Invalid GitHub item number".into());
    }
    let body = body.trim();
    if body.is_empty() {
        return Err("Comment cannot be empty".into());
    }
    Ok((kind, body))
}

pub(super) fn git_github_work_item_comment_for(
    root: &Path,
    repo: &str,
    kind: &str,
    number: i64,
    body: &str,
    in_reply_to: &str,
) -> Result<String, String> {
    let (kind, body) = github_comment_input(kind, number, body)?;
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let reply = in_reply_to.trim();
    if !reply.is_empty() {
        return git_github_review_reply_for(root, reply, body);
    }
    let number = number.to_string();
    with_temp_markdown(body, |path| {
        let output = gh_checked(
            root,
            &[
                kind,
                "comment",
                &number,
                "--repo",
                &repo,
                "--body-file",
                path,
            ],
        )?;
        github_url_from_output(&output, "GitHub did not return a comment URL")
    })
}

pub(super) fn git_github_review_reply_for(
    root: &Path,
    thread_id: &str,
    body: &str,
) -> Result<String, String> {
    if !valid_github_node_id(thread_id) {
        return Err("Invalid review thread".into());
    }
    let thread_field = format!("threadId={thread_id}");
    with_temp_markdown(body, |path| {
        let body_field = format!("body=@{path}");
        let json = gh_checked(
            root,
            &[
                "api",
                "graphql",
                "-f",
                &format!("query={GITHUB_REVIEW_REPLY_MUTATION}"),
                "-F",
                &thread_field,
                "-F",
                &body_field,
            ],
        )?;
        parse_github_review_reply_url(&json)
    })
}

fn with_temp_markdown(
    body: &str,
    run: impl FnOnce(&str) -> Result<String, String>,
) -> Result<String, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("vorkspace-comment-{stamp}.md"));
    std::fs::write(&path, body).map_err(|error| error.to_string())?;
    let path_str = path.to_string_lossy().into_owned();
    let result = run(&path_str);
    let _ = std::fs::remove_file(&path);
    result
}

fn valid_github_node_id(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty()
        && id.len() < 256
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '='))
}

fn github_url_from_output(output: &str, missing: &str) -> Result<String, String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("http://") || line.starts_with("https://"))
        .map(str::to_string)
        .ok_or_else(|| {
            if output.trim().is_empty() {
                missing.to_string()
            } else {
                output.trim().to_string()
            }
        })
}

fn parse_github_review_reply_url(json: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
    if let Some(message) = value
        .get("errors")
        .and_then(|errors| errors.as_array())
        .and_then(|errors| errors.first())
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        return Err(message.to_string());
    }
    let url = value
        .pointer("/data/addPullRequestReviewThreadReply/comment/url")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(url.to_string());
    }
    Err("GitHub did not return a comment URL".into())
}

fn split_github_repo(slug: &str) -> Result<(String, String), String> {
    let slug = slug.trim();
    let Some((owner, name)) = slug.split_once('/') else {
        return Err("GitHub did not return a repository".into());
    };
    let owner = owner.trim();
    let name = name.trim();
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || owner.chars().any(char::is_whitespace)
        || name.chars().any(char::is_whitespace)
    {
        return Err("GitHub did not return a repository".into());
    }
    Ok((owner.to_string(), name.to_string()))
}

#[derive(Deserialize)]
struct GithubGraphqlEnvelope {
    #[serde(default)]
    data: Option<GithubGraphqlData>,
    #[serde(default)]
    errors: Vec<GithubGraphqlError>,
}

#[derive(Deserialize)]
struct GithubGraphqlError {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct GithubGraphqlData {
    repository: Option<GithubGraphqlRepository>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlRepository {
    issue: Option<GithubGraphqlIssue>,
    pull_request: Option<GithubGraphqlPullRequest>,
}

#[derive(Deserialize)]
struct GithubGraphqlIssue {
    #[serde(default)]
    comments: GithubGraphqlNodes<GithubGraphqlComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlPullRequest {
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    base_ref_name: String,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    commits: GithubGraphqlNodes<GithubGraphqlCommitNode>,
    #[serde(default)]
    comments: GithubGraphqlNodes<GithubGraphqlComment>,
    #[serde(default)]
    reviews: GithubGraphqlNodes<GithubGraphqlReview>,
    #[serde(default)]
    review_threads: GithubGraphqlNodes<GithubGraphqlReviewThread>,
}

#[derive(Deserialize)]
struct GithubGraphqlCommitNode {
    commit: GithubGraphqlCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlCommit {
    #[serde(default)]
    oid: String,
    #[serde(default)]
    message_headline: String,
    #[serde(default)]
    committed_date: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    author: Option<GithubGraphqlCommitAuthor>,
}

#[derive(Deserialize)]
struct GithubGraphqlCommitAuthor {
    #[serde(default)]
    name: String,
    #[serde(default)]
    user: Option<GithubGraphqlActor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlNodes<T> {
    total_count: i64,
    nodes: Vec<T>,
}

impl<T> Default for GithubGraphqlNodes<T> {
    fn default() -> Self {
        Self {
            total_count: 0,
            nodes: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlComment {
    #[serde(default)]
    id: String,
    #[serde(default)]
    author: Option<GithubGraphqlActor>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    is_minimized: bool,
    #[serde(default)]
    path: String,
    #[serde(default)]
    line: Option<i64>,
    #[serde(default)]
    original_line: Option<i64>,
}

#[derive(Deserialize)]
struct GithubGraphqlReview {
    #[serde(default)]
    id: String,
    #[serde(default)]
    author: Option<GithubGraphqlActor>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    state: String,
    #[serde(default, rename = "submittedAt")]
    submitted_at: Option<String>,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubGraphqlReviewThread {
    #[serde(default)]
    id: String,
    #[serde(default)]
    is_resolved: bool,
    #[serde(default)]
    path: String,
    #[serde(default)]
    comments: GithubGraphqlNodes<GithubGraphqlComment>,
}

#[derive(Deserialize)]
struct GithubGraphqlActor {
    #[serde(default)]
    login: String,
}

fn parse_github_work_item_thread(json: &str, kind: &str) -> Result<GitHubWorkItemThread, String> {
    let envelope: GithubGraphqlEnvelope =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    let graphql_error = envelope
        .errors
        .iter()
        .map(|error| error.message.trim())
        .find(|message| !message.is_empty())
        .map(str::to_string);
    let Some(repository) = envelope.data.and_then(|data| data.repository) else {
        return Err(graphql_error.unwrap_or_else(|| "GitHub item not found".into()));
    };

    let mut comments = Vec::new();
    let mut commits = Vec::new();
    let mut truncated = false;
    let mut review_decision = String::new();
    let mut base_ref_name = String::new();
    let mut head_ref_name = String::new();

    if kind == "pr" {
        let Some(pull) = repository.pull_request else {
            return Err(graphql_error.unwrap_or_else(|| "GitHub pull request not found".into()));
        };
        review_decision = pull.review_decision.unwrap_or_default();
        base_ref_name = pull.base_ref_name;
        head_ref_name = pull.head_ref_name;
        truncated |= github_nodes_truncated(&pull.commits);
        commits.extend(pull.commits.nodes.into_iter().filter_map(|node| {
            let commit = node.commit;
            if commit.oid.trim().is_empty() || commit.committed_date.trim().is_empty() {
                return None;
            }
            let author = commit
                .author
                .map(|author| {
                    author
                        .user
                        .map(|user| user.login)
                        .filter(|login| !login.trim().is_empty())
                        .unwrap_or(author.name)
                })
                .unwrap_or_default();
            Some(GitHubWorkItemCommit {
                oid: commit.oid,
                message_headline: commit.message_headline,
                author,
                committed_date: commit.committed_date,
                url: commit.url,
            })
        }));
        truncated |= github_nodes_truncated(&pull.comments);
        comments.extend(
            pull.comments
                .nodes
                .into_iter()
                .filter_map(github_issue_comment),
        );
        truncated |= github_nodes_truncated(&pull.reviews);
        comments.extend(
            pull.reviews
                .nodes
                .into_iter()
                .filter_map(github_review_comment),
        );
        truncated |= github_nodes_truncated(&pull.review_threads);
        comments.extend(
            pull.review_threads
                .nodes
                .into_iter()
                .filter_map(github_review_thread),
        );
    } else {
        let Some(issue) = repository.issue else {
            return Err(graphql_error.unwrap_or_else(|| "GitHub issue not found".into()));
        };
        truncated |= github_nodes_truncated(&issue.comments);
        comments.extend(
            issue
                .comments
                .nodes
                .into_iter()
                .filter_map(github_issue_comment),
        );
    }

    comments.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(GitHubWorkItemThread {
        comments,
        commits,
        truncated,
        review_decision,
        base_ref_name,
        head_ref_name,
    })
}

fn github_nodes_truncated<T>(nodes: &GithubGraphqlNodes<T>) -> bool {
    (nodes.nodes.len() as i64) < nodes.total_count
}

fn github_actor_login(author: Option<GithubGraphqlActor>) -> String {
    author
        .map(|author| author.login)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn github_comment_id(kind: &str, id: &str, author: &str, created_at: &str) -> String {
    let id = id.trim();
    if !id.is_empty() {
        return id.to_string();
    }
    format!("{kind}:{author}:{created_at}")
}

fn github_issue_comment(comment: GithubGraphqlComment) -> Option<GitHubWorkItemComment> {
    github_mapped_comment(comment, "comment", "", false)
}

fn github_mapped_comment(
    comment: GithubGraphqlComment,
    kind: &str,
    fallback_path: &str,
    resolved: bool,
) -> Option<GitHubWorkItemComment> {
    if comment.is_minimized {
        return None;
    }
    let author = github_actor_login(comment.author);
    let created_at = comment.created_at.trim().to_string();
    let path = if comment.path.trim().is_empty() {
        fallback_path.trim().to_string()
    } else {
        comment.path.trim().to_string()
    };
    Some(GitHubWorkItemComment {
        id: github_comment_id(kind, &comment.id, &author, &created_at),
        kind: kind.to_string(),
        author_avatar_url: github_avatar_url(&author),
        author,
        body: comment.body,
        created_at,
        url: comment.url,
        state: String::new(),
        path,
        line: comment.line.or(comment.original_line),
        resolved,
        thread_id: String::new(),
        replies: Vec::new(),
    })
}

fn github_review_comment(review: GithubGraphqlReview) -> Option<GitHubWorkItemComment> {
    let state = review.state.trim().to_uppercase();
    if state.is_empty() || state == "PENDING" {
        return None;
    }
    let body = review.body.trim();
    if state == "COMMENTED" && body.is_empty() {
        return None;
    }
    let created_at = review
        .submitted_at
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    if created_at.is_empty() {
        return None;
    }
    let author = github_actor_login(review.author);
    Some(GitHubWorkItemComment {
        id: github_comment_id("review", &review.id, &author, &created_at),
        kind: "review".into(),
        author_avatar_url: github_avatar_url(&author),
        author,
        body: review.body,
        created_at,
        url: review.url,
        state,
        path: String::new(),
        line: None,
        resolved: false,
        thread_id: String::new(),
        replies: Vec::new(),
    })
}

fn github_review_thread(thread: GithubGraphqlReviewThread) -> Option<GitHubWorkItemComment> {
    let thread_id = thread.id.trim().to_string();
    let mut mapped = thread.comments.nodes.into_iter().filter_map(|comment| {
        github_mapped_comment(comment, "review_comment", &thread.path, thread.is_resolved)
    });
    let mut first = mapped.next()?;
    first.thread_id = thread_id.clone();
    first.replies = mapped
        .map(|mut reply| {
            reply.thread_id = thread_id.clone();
            reply
        })
        .collect();
    Some(first)
}

fn github_avatar_url(login: &str) -> String {
    let login = login.trim();
    if login.is_empty() {
        return String::new();
    }
    let mut encoded = String::with_capacity(login.len());
    for byte in login.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    format!("https://avatars.githubusercontent.com/{encoded}?s=64")
}

pub(super) fn git_github_pr_diff_for(
    root: &Path,
    repo: &str,
    number: i64,
) -> Result<GitHubPrDiff, String> {
    if number <= 0 {
        return Err("Invalid pull request number".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let number = number.to_string();
    let fields = "files,additions,deletions,baseRefOid,headRefOid";
    let json = gh_run(
        root,
        &["pr", "view", &number, "--repo", &repo, "--json", fields],
        false,
    )?;
    let mut diff = parse_github_pr_diff_meta(&json)?;
    let (base, head) = parse_github_pr_oids(&json)?;
    github_pr_hunks(
        &mut diff,
        &repo,
        &number,
        || git_pr_diff(root, &base, &head),
        |args| gh_run(root, args, true),
    )?;
    if diff.additions == 0 && diff.deletions == 0 {
        diff.additions = diff.files.iter().map(|file| file.additions).sum();
        diff.deletions = diff.files.iter().map(|file| file.deletions).sum();
    }
    Ok(diff)
}

// GitHub refuses the combined diff above its line limit. The paginated files
// endpoint still supplies individual patches, even without a local checkout.
// Prefer exact local commit objects when available: the API can omit big files.
fn github_pr_hunks(
    diff: &mut GitHubPrDiff,
    repo: &str,
    number: &str,
    local: impl FnOnce() -> Result<GitHubPrDiff, String>,
    mut run: impl FnMut(&[&str]) -> Result<String, String>,
) -> Result<(), String> {
    match run(&["pr", "diff", number, "--repo", repo]) {
        // gh pr view caps its file list at 100. At that boundary, reconstruct
        // the list too, even when GitHub accepts the combined patch.
        Ok(patch) if patch.len() <= MAX_PR_DIFF_BYTES && diff.files.len() < 100 => {
            diff.patch = patch;
            return Ok(());
        }
        Ok(_) => {}
        Err(error) if github_diff_too_large(&error) => {}
        Err(error) => return Err(error),
    }
    if let Ok(local) = local() {
        if !local.truncated {
            *diff = local;
            return Ok(());
        }
    }
    diff.files.clear();
    diff.patch.clear();
    // GitHub's files endpoint returns at most 3,000 files. Keep each response
    // under the existing CLI byte/time bounds; never cut a patch at the byte cap.
    for page in 1..=30 {
        let endpoint = format!("repos/{repo}/pulls/{number}/files?per_page=100&page={page}");
        let json = run(&["api", &endpoint])?;
        let rows: Vec<GitHubFilePatch> =
            serde_json::from_str(&json).map_err(|error| error.to_string())?;
        let count = rows.len();
        for file in rows {
            append_github_file_patch(diff, file);
        }
        if count < 100 {
            return Ok(());
        }
    }
    diff.truncated = true;
    Ok(())
}

fn github_diff_too_large(error: &str) -> bool {
    (error.contains("HTTP 406")
        && (error.contains("too_large") || error.contains("diff exceeded the maximum")))
        || error == "GitHub CLI output exceeds the 4 MiB limit"
}

#[derive(Deserialize)]
struct GitHubFilePatch {
    filename: String,
    #[serde(default)]
    previous_filename: Option<String>,
    status: String,
    additions: i64,
    deletions: i64,
    #[serde(default)]
    patch: Option<String>,
}

fn append_github_file_patch(diff: &mut GitHubPrDiff, file: GitHubFilePatch) {
    diff.files.push(GitHubPrFile {
        path: file.filename.clone(),
        additions: file.additions,
        deletions: file.deletions,
    });
    let Some(patch) = file.patch.filter(|patch| !patch.is_empty()) else {
        diff.truncated |= file.additions != 0 || file.deletions != 0;
        return;
    };
    // Compare API totals with actual hunk lines so omitted/truncated provider
    // patches are never presented as a complete review.
    let mut in_hunk = false;
    let (mut additions, mut deletions) = (0, 0);
    for line in patch.lines() {
        if line.starts_with("@@ ") {
            in_hunk = true;
        } else if in_hunk {
            additions += i64::from(line.starts_with('+'));
            deletions += i64::from(line.starts_with('-'));
        }
    }
    diff.truncated |= additions != file.additions || deletions != file.deletions;
    let old = file.previous_filename.as_deref().unwrap_or(&file.filename);
    // JSON quoting keeps spaces, quotes, newlines and Unicode inside one header.
    let old = serde_json::to_string(&format!("a/{old}")).unwrap();
    let new = serde_json::to_string(&format!("b/{}", file.filename)).unwrap();
    let block = format!(
        "diff --git {old} {new}\n--- {}\n+++ {}\n{patch}\n",
        if file.status == "added" {
            "/dev/null"
        } else {
            &old
        },
        if file.status == "removed" {
            "/dev/null"
        } else {
            &new
        },
    );
    if diff.patch.len() + block.len() > MAX_PR_DIFF_BYTES {
        diff.truncated = true;
    } else {
        diff.patch.push_str(&block);
    }
}

fn parse_github_pr_oids(json: &str) -> Result<(String, String), String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        base_ref_oid: String,
        head_ref_oid: String,
    }
    let row: Row = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let base = row.base_ref_oid.trim();
    let head = row.head_ref_oid.trim();
    if base.is_empty() || head.is_empty() {
        return Err("Pull request is missing base or head commit".into());
    }
    if [base, head].iter().any(|oid| {
        !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err("Pull request has an invalid commit ID".into());
    }
    Ok((base.to_string(), head.to_string()))
}

pub(super) fn git_github_pr_checks_for(
    root: &Path,
    repo: &str,
    number: i64,
) -> Result<GitHubPrChecks, String> {
    let args = github_pr_checks_args(repo, number)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let json = gh_checked(root, &refs)?;
    parse_github_pr_checks(&json)
}

fn github_pr_checks_args(repo: &str, number: i64) -> Result<Vec<String>, String> {
    if number <= 0 {
        return Err("GitHub pull request number must be positive".into());
    }
    let (owner, name) = split_github_repo(repo)?;
    let repo = format!("{owner}/{name}");
    let number = number.to_string();
    Ok(vec![
        "pr".into(),
        "view".into(),
        number,
        "--repo".into(),
        repo,
        "--json".into(),
        "headRefOid,statusCheckRollup".into(),
    ])
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitHubStatusCheckRow {
    #[serde(default, rename = "__typename")]
    typename: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    context: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    workflow_name: String,
    #[serde(default)]
    details_url: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    completed_at: Option<String>,
}

fn parse_github_pr_checks(json: &str) -> Result<GitHubPrChecks, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        #[serde(default)]
        head_ref_oid: String,
        #[serde(default)]
        status_check_rollup: Option<Vec<GitHubStatusCheckRow>>,
    }
    let row: Row = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let head_oid = row.head_ref_oid.trim().to_string();
    if head_oid.is_empty() {
        return Err("Pull request is missing head commit".into());
    }
    Ok(GitHubPrChecks {
        head_oid,
        checks: row
            .status_check_rollup
            .unwrap_or_default()
            .into_iter()
            .map(github_pr_check_from_row)
            .collect(),
    })
}

fn github_pr_check_from_row(row: GitHubStatusCheckRow) -> GitHubPrCheck {
    let is_status_context = if row.typename.is_empty() {
        !row.context.is_empty()
    } else {
        row.typename.eq_ignore_ascii_case("StatusContext")
    };
    if is_status_context {
        return GitHubPrCheck {
            name: row.context,
            workflow: String::new(),
            state: github_check_conclusion_state(&row.state),
            url: row.target_url.filter(|url| !url.trim().is_empty()),
            started_at: row.created_at,
            completed_at: None,
        };
    }
    GitHubPrCheck {
        name: row.name,
        workflow: row.workflow_name,
        state: github_check_state(&row.status, row.conclusion.as_deref().unwrap_or_default()),
        url: row.details_url.filter(|url| !url.trim().is_empty()),
        started_at: row.started_at,
        completed_at: row.completed_at,
    }
}

/// An unfinished CheckRun reports its execution state instead of a stale
/// conclusion. Only a completed or entirely missing status trusts the
/// conclusion; any other nonempty status stays unknown.
fn github_check_state(status: &str, conclusion: &str) -> String {
    match status.trim().to_ascii_uppercase().as_str() {
        "QUEUED" | "IN_PROGRESS" | "PENDING" | "WAITING" | "REQUESTED" => {
            return "pending".into();
        }
        "COMPLETED" | "" => return github_check_conclusion_state(conclusion),
        _ => {}
    }
    "unknown".into()
}

fn github_check_conclusion_state(value: &str) -> String {
    match value.trim().to_ascii_uppercase().as_str() {
        "SUCCESS" => "pass".into(),
        "FAILURE" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => "fail".into(),
        "QUEUED" | "IN_PROGRESS" | "PENDING" | "WAITING" | "REQUESTED" => "pending".into(),
        "NEUTRAL" | "SKIPPED" => "skipping".into(),
        "CANCELLED" => "cancel".into(),
        _ => "unknown".into(),
    }
}

fn parse_github_pr_diff_meta(json: &str) -> Result<GitHubPrDiff, String> {
    #[derive(Deserialize)]
    struct FileRow {
        path: String,
        #[serde(default)]
        additions: i64,
        #[serde(default)]
        deletions: i64,
    }
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        additions: i64,
        #[serde(default)]
        deletions: i64,
        #[serde(default)]
        files: Vec<FileRow>,
    }
    let row: Row = serde_json::from_str(json).map_err(|error| error.to_string())?;
    Ok(GitHubPrDiff {
        additions: row.additions,
        deletions: row.deletions,
        files: row
            .files
            .into_iter()
            .map(|file| GitHubPrFile {
                path: file.path,
                additions: file.additions,
                deletions: file.deletions,
            })
            .collect(),
        patch: String::new(),
        truncated: false,
    })
}

fn parse_github_work_items(
    json: &str,
    kind: &str,
    repo: &str,
) -> Result<Vec<GitHubWorkItem>, String> {
    #[derive(Deserialize)]
    struct RowLabel {
        name: String,
        #[serde(default)]
        color: String,
    }
    #[derive(Deserialize)]
    struct RowAssignee {
        login: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        number: i64,
        title: String,
        url: String,
        state: String,
        #[serde(default)]
        state_reason: String,
        #[serde(default)]
        created_at: String,
        #[serde(default)]
        updated_at: String,
        #[serde(default)]
        labels: Vec<RowLabel>,
        #[serde(default)]
        assignees: Vec<RowAssignee>,
        #[serde(default)]
        is_draft: bool,
    }
    let rows: Vec<Row> = serde_json::from_str(json).map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| GitHubWorkItem {
            kind: kind.to_string(),
            number: row.number,
            title: row.title,
            url: row.url,
            state: row.state.to_lowercase(),
            state_reason: row.state_reason.to_lowercase(),
            created_at: row.created_at,
            updated_at: row.updated_at,
            labels: row
                .labels
                .into_iter()
                .map(|label| GitHubLabel {
                    name: label.name,
                    color: label.color,
                })
                .collect(),
            assignees: row
                .assignees
                .into_iter()
                .map(|assignee| GitHubAssignee {
                    avatar_url: github_avatar_url(&assignee.login),
                    login: assignee.login,
                })
                .collect(),
            draft: row.is_draft,
            repo: repo.to_string(),
        })
        .collect())
}

fn parse_github_work_item(json: &str, kind: &str, repo: &str) -> Result<GitHubWorkItem, String> {
    let wrapped = format!("[{json}]");
    parse_github_work_items(&wrapped, kind, repo)?
        .into_iter()
        .next()
        .ok_or_else(|| "GitHub did not return a work item".into())
}

fn gh_checked(root: &Path, args: &[&str]) -> Result<String, String> {
    super::run_gh(root, args, false)
}
fn gh_run(root: &Path, args: &[&str], allow_empty: bool) -> Result<String, String> {
    super::run_gh(root, args, allow_empty)
}
fn git_pr_diff(root: &Path, base: &str, head: &str) -> Result<GitHubPrDiff, String> {
    let range = format!("{base}...{head}");
    let stats = super::run_git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--numstat",
            "-z",
            &range,
            "--",
        ],
    )?;
    let files = parse_git_pr_files(&stats)?;
    let mut diff = GitHubPrDiff {
        additions: files.iter().map(|file| file.additions).sum(),
        deletions: files.iter().map(|file| file.deletions).sum(),
        files,
        patch: String::new(),
        truncated: false,
    };
    let output = super::run_git(
        root,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-U3",
            &range,
            "--",
        ],
    );
    match output {
        Ok(patch) if patch.len() <= MAX_PR_DIFF_BYTES => diff.patch = patch,
        Ok(_) => diff.truncated = true,
        Err(error) if github_diff_too_large(&error) => diff.truncated = true,
        Err(error) => return Err(error),
    }
    Ok(diff)
}

fn parse_git_pr_files(stats: &str) -> Result<Vec<GitHubPrFile>, String> {
    let mut records = stats.split_terminator('\0');
    let mut files = Vec::new();
    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, '\t');
        let mut count = || match fields.next() {
            Some("-") => Ok(0),
            Some(n) => n.parse::<i64>().map_err(|_| "Invalid Git diff statistics"),
            None => Err("Incomplete Git diff statistics"),
        };
        let additions = count()?;
        let deletions = count()?;
        let path = fields.next().ok_or("Missing Git diff path")?;
        // With -z, renames have an empty path followed by old and new names.
        let path = if path.is_empty() {
            records.next().ok_or("Missing Git rename source")?;
            records.next().ok_or("Missing Git rename destination")?
        } else {
            path
        };
        files.push(GitHubPrFile {
            path: path.into(),
            additions,
            deletions,
        });
    }
    Ok(files)
}

#[cfg(test)]
mod diff_tests {
    use super::*;
    use serde_json::{json, Value};

    const TOO_LARGE: &str = "could not find pull request diff: HTTP 406: Sorry, the diff exceeded the maximum number of lines (20000)\nPullRequest.diff too_large";

    fn no_local() -> Result<GitHubPrDiff, String> {
        Err("No local objects".into())
    }

    fn metadata() -> GitHubPrDiff {
        parse_github_pr_diff_meta(
            r#"{"additions":101,"deletions":0,"files":[{"path":"incomplete-metadata"}]}"#,
        )
        .unwrap()
    }

    fn file(path: &str) -> Value {
        json!({"filename":path, "status":"added", "additions":1, "deletions":0,
            "patch":"@@ -0,0 +1 @@\n+hello"})
    }

    #[test]
    fn local_stats_include_every_file_and_preserve_rename_and_binary_paths() {
        let files = parse_git_pr_files(concat!(
            "1\t2\tspace\tname.rs\0-\t-\tbinary.png\0",
            "0\t0\t\0old.rs\0renamed\nλ.rs\0"
        ))
        .unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "space\tname.rs");
        assert_eq!(files[1].additions, 0);
        assert_eq!(files[2].path, "renamed\nλ.rs");
        for invalid in ["1\t0", "1\t0\t\0old\0", "bad\t0\tfile\0"] {
            assert!(parse_git_pr_files(invalid).is_err());
        }
        assert!(parse_github_pr_oids(r#"{"baseRefOid":"--bad","headRefOid":"--bad"}"#).is_err());
    }

    #[test]
    fn local_reconstruction_uses_merge_base_without_touching_checkout_or_index() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        std::fs::write(root.join("a.rs"), "before\n").unwrap();
        std::fs::write(root.join("deleted.rs"), "gone\n").unwrap();
        git(&["add", "."]);
        let commit = [
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "test",
        ];
        git(&commit);
        let base = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-qb", "review"]);
        std::fs::write(root.join("a.rs"), "after\n").unwrap();
        std::fs::remove_file(root.join("deleted.rs")).unwrap();
        std::fs::write(root.join("new.rs"), "new\n").unwrap();
        git(&["add", "-A"]);
        git(&commit);
        let head = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-qb", "advanced", &base]);
        std::fs::write(root.join("outside.rs"), "outside PR\n").unwrap();
        git(&["add", "."]);
        git(&commit);
        let advanced = git(&["rev-parse", "HEAD"]);
        std::fs::write(root.join("a.rs"), "staged\n").unwrap();
        git(&["add", "a.rs"]);
        std::fs::write(root.join("a.rs"), "unsaved workspace\n").unwrap();
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let mut diff = metadata();
        github_pr_hunks(
            &mut diff,
            "acme/app",
            "49",
            || git_pr_diff(root, &advanced, &head),
            |args| {
                assert_eq!(
                    args[0], "pr",
                    "local reconstruction must not call the files API"
                );
                Err(TOO_LARGE.into())
            },
        )
        .unwrap();
        assert_eq!(
            diff.files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["a.rs", "deleted.rs", "new.rs"]
        );
        assert_eq!((diff.additions, diff.deletions), (2, 2));
        assert!(!diff.truncated);
        assert!(diff.patch.contains("+after"));
        assert!(!diff.patch.contains("outside PR"));
        assert_eq!(
            git_pr_diff(root, &advanced, &head).unwrap().files,
            diff.files
        );
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(git(&["rev-parse", "HEAD"]), advanced);
        assert_eq!(
            std::fs::read_to_string(root.join("a.rs")).unwrap(),
            "unsaved workspace\n"
        );
    }

    #[test]
    fn accepted_patch_with_capped_metadata_still_loads_the_complete_file_list() {
        let mut diff = metadata();
        diff.files = vec![diff.files[0].clone(); 100];
        github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
            if args[0] == "pr" {
                Ok("accepted patch".into())
            } else {
                Ok(json!([file("complete.rs")]).to_string())
            }
        })
        .unwrap();
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path, "complete.rs");
    }

    #[test]
    fn ordinary_hunks_keep_the_fast_path_and_unrelated_errors_propagate() {
        let mut diff = metadata();
        github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
            assert_eq!(args, ["pr", "diff", "49", "--repo", "acme/app"]);
            Ok("ordinary patch".into())
        })
        .unwrap();
        assert_eq!(diff.patch, "ordinary patch");
        assert_eq!(diff.files[0].path, "incomplete-metadata");
        for error in [
            "HTTP 401: Bad credentials",
            "HTTP 406: Unsupported media type",
            "lookup timed out",
        ] {
            assert_eq!(
                github_pr_hunks(&mut diff, "acme/app", "49", no_local, |_| Err(error.into()))
                    .unwrap_err(),
                error
            );
        }
    }

    #[test]
    fn oversized_combined_diff_uses_all_file_pages_and_preserves_totals() {
        let mut diff = metadata();
        let first = (0..100)
            .map(|i| file(&format!("file-{i}.rs")))
            .collect::<Vec<_>>();
        let responses = [
            Err(TOO_LARGE.into()),
            Ok(serde_json::to_string(&first).unwrap()),
            Ok(json!([file("last.rs")]).to_string()),
        ];
        let mut responses = responses.into_iter();
        let mut calls = Vec::new();
        github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
            calls.push(args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>());
            responses.next().expect("unexpected request")
        })
        .unwrap();
        assert!(responses.next().is_none());
        assert_eq!(
            calls[1],
            ["api", "repos/acme/app/pulls/49/files?per_page=100&page=1"]
        );
        assert_eq!(
            calls[2],
            ["api", "repos/acme/app/pulls/49/files?per_page=100&page=2"]
        );
        assert_eq!(diff.files.len(), 101);
        assert_eq!(diff.additions, 101);
        assert_eq!(diff.files.last().unwrap().path, "last.rs");
        assert!(diff.patch.contains("+++ \"b/last.rs\"\n"));
        assert!(!diff.truncated);
    }

    #[test]
    fn missing_and_incomplete_patches_are_reported_without_losing_other_files() {
        let mut diff = metadata();
        let rows = json!([
            {"filename":"missing.rs","status":"modified","additions":1,"deletions":0},
            {"filename":"partial.rs","status":"modified","additions":2,"deletions":0,"patch":"@@ -0,0 +1,2 @@\n+one"},
            file("present.rs")
        ]);
        github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
            if args[0] == "pr" {
                Err(TOO_LARGE.into())
            } else {
                Ok(rows.to_string())
            }
        })
        .unwrap();
        assert!(diff.truncated);
        assert_eq!(diff.files.len(), 3);
        assert!(!diff.patch.contains("missing.rs"));
        assert!(diff.patch.contains("+++ \"b/present.rs\""));
    }

    #[test]
    fn byte_limit_skips_a_large_file_and_retains_later_small_patches() {
        let mut diff = metadata();
        let mut big = file("big.rs");
        big["patch"] = json!(format!("@@ -0,0 +1 @@\n+{}", "x".repeat(MAX_PR_DIFF_BYTES)));
        let rows = json!([big, file("small.rs")]).to_string();
        for oversized_output in [false, true] {
            github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
                if args[0] == "api" {
                    Ok(rows.clone())
                } else if oversized_output {
                    Err("GitHub CLI output exceeds the 4 MiB limit".into())
                } else {
                    Ok("x".repeat(MAX_PR_DIFF_BYTES + 1))
                }
            })
            .unwrap();
            assert!(diff.truncated);
            assert_eq!(diff.files.len(), 2);
            assert!(diff.patch.len() <= MAX_PR_DIFF_BYTES);
            assert!(!diff.patch.contains("big.rs"));
            assert!(diff.patch.contains("small.rs"));
        }
    }

    #[test]
    fn renamed_and_deleted_headers_preserve_paths_and_hunk_content() {
        let mut diff = metadata();
        let name = "space \"quoted\"/λ\nfile.rs";
        let rows = json!([
            {"filename":name,"previous_filename":"old.rs","status":"renamed","additions":1,"deletions":1,"patch":"@@ -1 +1 @@\n-old\n+new"},
            {"filename":"deleted.rs","status":"removed","additions":0,"deletions":1,"patch":"@@ -1 +0,0 @@\n-gone"}
        ]).to_string();
        github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
            if args[0] == "pr" {
                Err(TOO_LARGE.into())
            } else {
                Ok(rows.clone())
            }
        })
        .unwrap();
        assert_eq!(diff.files[0].path, name);
        assert!(diff.patch.contains(&format!(
            "--- \"a/old.rs\"\n+++ {}\n",
            serde_json::to_string(&format!("b/{name}")).unwrap()
        )));
        assert!(diff
            .patch
            .contains("--- \"a/deleted.rs\"\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone"));
        assert!(!diff.truncated);
    }

    #[test]
    fn file_api_cap_is_explicit_and_page_errors_do_not_become_success() {
        let mut diff = metadata();
        let page = serde_json::to_string(
            &(0..100)
                .map(|i| file(&format!("{i}.rs")))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let mut calls = 0;
        github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
            calls += 1;
            if args[0] == "pr" {
                Err(TOO_LARGE.into())
            } else {
                Ok(page.clone())
            }
        })
        .unwrap();
        assert_eq!(calls, 31);
        assert_eq!(diff.files.len(), 3000);
        assert!(diff.truncated);
        assert_eq!(
            github_pr_hunks(&mut diff, "acme/app", "49", no_local, |args| {
                Err(if args[0] == "pr" {
                    TOO_LARGE
                } else {
                    "HTTP 403: rate limited"
                }
                .into())
            })
            .unwrap_err(),
            "HTTP 403: rate limited"
        );
    }
}
