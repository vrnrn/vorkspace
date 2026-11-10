//! Settings discovery runs on a worker; rendering never probes CLIs or Git.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use vorkspace_core::{
    git,
    provider::{self, Kind},
};
#[derive(Clone, Debug)]
pub(super) struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub scope: String,
    pub provider: String,
}
#[derive(Clone, Debug)]
pub(super) struct McpServer {
    pub name: String,
    pub provider: String,
    pub scope: String,
    pub path: PathBuf,
    pub transport: String,
    pub status: String,
}
#[derive(Default)]
pub(super) struct Discovery {
    pub binaries: Vec<(Kind, Option<PathBuf>)>,
    pub skills: Vec<Skill>,
    pub mcp: Vec<McpServer>,
    pub mcp_error: Option<String>,
    pub worktrees: ResultTrees,
}
#[derive(Clone, Debug)]
pub(super) struct Tree {
    pub tree: git::Worktree,
    pub assessment: vorkspace_core::worktree_management::Assessment,
}
#[derive(Default)]
pub(super) struct ResultTrees {
    pub trees: Vec<Tree>,
    pub branches: Arc<[git::Branch]>,
    pub default_root: PathBuf,
    pub main: String,
    pub error: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiscoveryPage {
    Providers,
    Skills,
    Mcp,
    Worktrees,
}

pub(super) fn discover(page: DiscoveryPage, cwd: &str) -> Discovery {
    let mut data = Discovery::default();
    match page {
        DiscoveryPage::Providers => {
            data.binaries = Kind::ALL
                .into_iter()
                .map(|kind| (kind, provider::resolve(kind)))
                .collect();
        }
        DiscoveryPage::Skills => data.skills = skills(cwd),
        DiscoveryPage::Mcp => match mcp(cwd) {
            Ok(servers) => data.mcp = servers,
            Err(error) => data.mcp_error = Some(error),
        },
        DiscoveryPage::Worktrees => data.worktrees = worktrees(cwd),
    }
    data
}

fn skills(cwd: &str) -> Vec<Skill> {
    vorkspace_core::skills::list_skills(cwd.into(), None)
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            let path = PathBuf::from(s.path);

            Skill {
                name: s.name,
                description: s.description,
                path,
                scope: s.scope,
                provider: s.source,
            }
        })
        .collect()
}

fn mcp(cwd: &str) -> Result<Vec<McpServer>, String> {
    Ok(vorkspace_core::mcp::mcp_discover(cwd.into())?
        .into_iter()
        .map(|server| McpServer {
            name: server.name,
            provider: server.provider,
            scope: server.scope,
            path: server.config_path.into(),
            transport: server.transport,
            status: if server.enabled {
                "Configured"
            } else {
                "Disabled"
            }
            .into(),
        })
        .collect())
}

fn worktrees(cwd: &str) -> ResultTrees {
    let mut trees = ResultTrees::default();
    if cwd.is_empty() {
        return trees;
    }
    match vorkspace_model::git_cache::load(Path::new(cwd)) {
        Ok(snapshot) => {
            if let Some(main) = snapshot.worktrees.first() {
                trees.main = main.path.clone();
                trees.default_root =
                    vorkspace_core::worktree_management::default_root(Path::new(&main.path));
            }
            trees.branches = snapshot.branches;
            trees.trees = snapshot
                .worktrees
                .iter()
                .filter(|tree| !tree.main)
                .cloned()
                .map(|tree| {
                    let assessment =
                        vorkspace_core::worktree_management::assess(Path::new(cwd), &tree);
                    Tree { tree, assessment }
                })
                .collect();
        }
        Err(error) => trees.error = Some(error),
    }
    trees
}
