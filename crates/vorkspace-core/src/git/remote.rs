//! Branch and worktree RPCs shared with the original remote project pickers.
use super::{Branch, Snapshot, Worktree};
use crate::remote::{default_connections, parse_remote_path, remote_path, Connections, RemotePath};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};

#[derive(Deserialize)]
struct Project {
    id: String,
    cwd: String,
}
#[derive(Deserialize)]
struct Trees {
    worktrees: Vec<Worktree>,
}
#[derive(Deserialize)]
struct Branches {
    current: Option<String>,
    branches: Vec<String>,
    remotes: Vec<RemoteBranch>,
}
#[derive(Deserialize)]
struct RemoteBranch {
    name: String,
    remote: String,
}
#[derive(Deserialize)]
struct Checkout {
    #[serde(flatten)]
    branches: Branches,
    linked: bool,
    worktrees: Vec<Worktree>,
}

struct Context {
    connections: Arc<Connections>,
    project: String,
    environment: String,
    cwd: String,
}
impl Context {
    fn request(&self, method: &str, mut params: Value) -> Result<Value, String> {
        params["projectId"] = json!(self.project);
        params["cwd"] = json!(self.cwd);
        self.connections
            .request_for_environment(&self.environment, method, params)
    }
    fn trees(&self) -> Result<Trees, String> {
        decode(self.request("git.worktrees", json!({}))?)
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("Invalid remote Git response: {error}"))
}
fn context(cwd: &Path) -> Result<Context, String> {
    let remote = parse_remote_path(&cwd.to_string_lossy()).ok_or("Invalid remote working copy")?;
    resolve_project(default_connections()?, remote)
}
fn resolve_project(connections: Arc<Connections>, remote: RemotePath) -> Result<Context, String> {
    let projects: Vec<Project> = decode(connections.request_for_environment(
        &remote.environment_id,
        "projects.list",
        json!({}),
    )?)?;
    let mut project = projects
        .iter()
        .find(|project| key(&project.cwd) == key(&remote.host_path))
        .map(|project| project.id.clone());
    // Worktrees commonly live beside, rather than under, their main checkout.
    if project.is_none() {
        for candidate in &projects {
            let listed = connections.request_for_environment(
                &remote.environment_id,
                "git.worktrees",
                json!({"projectId":candidate.id}),
            );
            if let Ok(listed) = listed.and_then(decode::<Trees>) {
                if listed
                    .worktrees
                    .iter()
                    .any(|tree| !tree.missing && key(&tree.path) == key(&remote.host_path))
                {
                    project = Some(candidate.id.clone());
                    break;
                }
            }
        }
    }
    Ok(Context {
        connections,
        project: project.ok_or("Choose an available worktree of this project")?,
        environment: remote.environment_id,
        cwd: remote.host_path,
    })
}
fn key(path: &str) -> String {
    crate::files::path_key(path)
}

pub(super) fn snapshot(cwd: &Path) -> Result<Snapshot, String> {
    let remote = parse_remote_path(&cwd.to_string_lossy()).ok_or("Invalid remote working copy")?;
    read_checkout(default_connections()?, remote)
}

fn read_checkout(connections: Arc<Connections>, remote: RemotePath) -> Result<Snapshot, String> {
    let checkout: Checkout = match connections.request_for_environment(
        &remote.environment_id,
        "git.snapshot",
        json!({"cwd":remote.host_path}),
    ) {
        Ok(value) => decode(value)?,
        Err(error) if error == "Host rejected request: Unsupported host method" => {
            let context = resolve_project(connections, remote.clone())?;
            let branches = decode(context.request("git.branches", json!({}))?)?;
            let Trees { worktrees } = context.trees()?;
            let cwd = key(&remote.host_path);
            let linked = worktrees
                .iter()
                .any(|tree| !tree.main && key(&tree.path) == cwd);
            Checkout {
                branches,
                linked,
                worktrees,
            }
        }
        Err(error) => return Err(error),
    };
    let Branches {
        current,
        branches,
        remotes,
    } = checkout.branches;
    Ok(Snapshot {
        detached: current.is_none(),
        current,
        linked: checkout.linked,
        branches: branches
            .into_iter()
            .map(|name| Branch { name, remote: None })
            .chain(remotes.into_iter().map(|branch| Branch {
                name: branch.name,
                remote: Some(branch.remote),
            }))
            .collect(),
        worktrees: scoped_trees(checkout.worktrees, &remote.environment_id).into(),
    })
}

fn scoped_trees(mut worktrees: Vec<Worktree>, environment: &str) -> Vec<Worktree> {
    for tree in &mut worktrees {
        tree.path = remote_path(environment, &tree.path);
    }
    worktrees
}

pub(super) fn worktrees(cwd: &Path) -> Result<Vec<Worktree>, String> {
    let context = context(cwd)?;
    Ok(scoped_trees(
        context.trees()?.worktrees,
        &context.environment,
    ))
}
pub(super) fn checkout(cwd: &Path, branch: &Branch) -> Result<(), String> {
    context(cwd)?.request(
        "git.switch",
        json!({"branch":branch.name,"remote":branch.remote}),
    )?;
    Ok(())
}
pub(super) fn create_branch(cwd: &Path, branch: &str) -> Result<(), String> {
    context(cwd)?.request("git.createBranch", json!({"branch":branch}))?;
    Ok(())
}
pub(super) fn create_worktree(
    cwd: &Path,
    branch: &str,
    base: Option<&str>,
    existing: bool,
) -> Result<Worktree, String> {
    let context = context(cwd)?;
    let mut tree: Worktree = decode(context.request(
        "git.worktreeCreate",
        json!({"branch":branch,"base":base.unwrap_or("HEAD"),"existing":existing}),
    )?)?;
    tree.path = remote_path(&context.environment, &tree.path);
    Ok(tree)
}
