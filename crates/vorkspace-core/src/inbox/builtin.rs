//! Existing connector implementations, executed exclusively by the bundled plugins.
use super::*;
pub struct BuiltinService {
    pub data_dir: PathBuf,
    pub cwd: PathBuf,
}
impl BuiltinService {
    pub fn status(&self, provider: &str) -> Result<Status, String> {
        match provider {
            "jira" => convert(jira::jira_status(self.data_dir.clone())?),
            "gitlab" => convert(gitlab::gitlab_status(self.data_dir.clone())?),
            "github" => {
                let installed = crate::scm::resolve_gui_binary("gh").is_some();
                let authenticated = installed
                    && run_gh(
                        &self.cwd,
                        &["auth", "status", "--active", "--hostname", "github.com"],
                        true,
                    )
                    .is_ok();
                Ok(Status {
                    installed,
                    authenticated,
                    connected: authenticated,
                    ..Default::default()
                })
            }
            _ => Err("Unsupported connector".into()),
        }
    }
    pub fn connect(
        &self,
        provider: &str,
        url: String,
        email: String,
        token: String,
    ) -> Result<Status, String> {
        match provider {
            "jira" => convert(jira::jira_set_config(
                self.data_dir.clone(),
                url,
                email,
                token,
            )?),
            "gitlab" => convert(gitlab::gitlab_set_config(
                self.data_dir.clone(),
                url,
                token,
            )?),
            "github" => self.status("github"),
            _ => Err("Unsupported connector".into()),
        }
    }
    pub fn list(&self, projects: &[PathBuf], state: &ViewState) -> Snapshot {
        let mut snapshot = Snapshot::default();
        for provider in state.connectors.enabled_providers() {
            let filters = state.filters_for(provider);
            let limit = if filters.fetch_state() == "all" {
                100
            } else {
                40
            };
            let status = match self.status(provider) {
                Ok(status) => status,
                Err(error) => {
                    snapshot.errors.insert(provider.into(), error);
                    continue;
                }
            };
            let connected = status.connected;
            snapshot.connections.insert(provider.into(), status);
            if !connected {
                continue;
            }
            let result: Result<Vec<Item>, String> = (|| {
                if provider == "jira" {
                    if let Some(jql) = state.jira_filter.custom_jql() {
                        return convert(jira::jira_list_issues(
                            self.data_dir.clone(),
                            false,
                            "all".into(),
                            vec![],
                            Some(100),
                            Some(jql.into()),
                        )?);
                    }
                    let projects = jira::jira_list_projects(self.data_dir.clone())?;
                    let visible = projects
                        .iter()
                        .filter(|p| !state.hidden_jira_projects.contains(&p.id))
                        .map(|p| p.id.clone())
                        .collect::<Vec<_>>();
                    snapshot.jira_projects = convert(projects)?;
                    if !state.hidden_jira_projects.is_empty() && visible.is_empty() {
                        return Ok(vec![]);
                    }
                    return convert(jira::jira_list_issues(
                        self.data_dir.clone(),
                        filters.assigned_to_me,
                        filters.fetch_state().into(),
                        if state.hidden_jira_projects.is_empty() {
                            vec![]
                        } else {
                            visible
                        },
                        Some(limit),
                        None,
                    )?);
                }
                let mut items = Vec::new();
                let mut visited = HashSet::new();
                if filters.assigned_to_me && provider == "gitlab" {
                    for kind in ["issue", "pr"] {
                        let rows: Vec<Item> = convert(gitlab::gitlab_list_todos(
                            self.data_dir.clone(),
                            kind.into(),
                            Some(limit),
                        )?)?;
                        items.extend(rows.into_iter().map(|mut item| {
                            item.provider = provider.into();
                            item
                        }));
                    }
                    return Ok(items);
                }
                for project in projects {
                    let repos = match provider {
                        "github" => github::git_github_repositories_for(project),
                        "gitlab" => gitlab::gitlab_repo(
                            self.data_dir.clone(),
                            project.to_string_lossy().into_owned(),
                        )
                        .map(|r| vec![r]),
                        _ => Err("Unsupported connector".into()),
                    };
                    let Ok(repos) = repos else {
                        continue;
                    };
                    for repo in repos {
                        if !visited.insert(repo.to_lowercase()) {
                            continue;
                        }
                        for kind in ["issue", "pr"] {
                            let rows: Vec<Item> = match provider {
                                "github" => convert(github::git_github_work_items_for(
                                    project,
                                    &repo,
                                    kind,
                                    filters.assigned_to_me,
                                    filters.fetch_state(),
                                    "",
                                    limit,
                                )?)?,
                                "gitlab" => convert(gitlab::gitlab_list_work_items(
                                    self.data_dir.clone(),
                                    project.to_string_lossy().into_owned(),
                                    kind.into(),
                                    false,
                                    filters.fetch_state().into(),
                                    Some(limit),
                                )?)?,
                                _ => return Err("Unsupported connector".into()),
                            };
                            items.extend(rows.into_iter().map(|mut item| {
                                item.provider = provider.into();
                                item.project_path = project.to_string_lossy().into_owned();
                                item
                            }));
                        }
                    }
                }
                Ok(items)
            })();
            match result {
                Ok(items) => snapshot.items.extend(items),
                Err(error) => {
                    snapshot.errors.insert(provider.into(), error);
                }
            }
        }
        snapshot.items.sort_by(|a, b| {
            b.updated_ms()
                .cmp(&a.updated_ms())
                .then_with(|| a.title.cmp(&b.title))
        });
        let mut seen = HashSet::new();
        snapshot.items.retain(|i| seen.insert(i.key()));
        snapshot
    }
    pub fn details(&self, item: &Item) -> Result<Details, String> {
        match item.provider.as_str() {
            "jira" => convert(jira::jira_issue_details(
                self.data_dir.clone(),
                item.identifier.clone(),
            )?),
            "gitlab" => convert(gitlab::gitlab_work_item_details(
                self.data_dir.clone(),
                item.repo.clone(),
                item.kind.clone(),
                item.number,
            )?),
            "github" => convert(github::git_github_work_item_details_for(
                self.project(item),
                &item.repo,
                &item.kind,
                item.number,
            )?),
            _ => Err("Unsupported connector".into()),
        }
    }
    fn project<'a>(&'a self, item: &'a Item) -> &'a Path {
        if item.project_path.is_empty() {
            &self.cwd
        } else {
            Path::new(&item.project_path)
        }
    }
    pub fn thread(&self, item: &Item) -> Result<Thread, String> {
        match item.provider.as_str() {
            "jira" => convert(jira::jira_issue_thread(
                self.data_dir.clone(),
                item.identifier.clone(),
            )?),
            "gitlab" => convert(gitlab::gitlab_work_item_thread(
                self.data_dir.clone(),
                item.repo.clone(),
                item.kind.clone(),
                item.number,
            )?),
            "github" => convert(github::git_github_work_item_thread_for(
                self.project(item),
                &item.repo,
                &item.kind,
                item.number,
            )?),
            _ => Err("Unsupported connector".into()),
        }
    }
    pub fn comment(
        &self,
        item: &Item,
        body: String,
        reply_to: String,
    ) -> Result<CommentReceipt, String> {
        match item.provider.as_str() {
            "jira" => {
                jira::jira_issue_comment(self.data_dir.clone(), item.identifier.clone(), body)
            }
            "gitlab" => gitlab::gitlab_work_item_comment(
                self.data_dir.clone(),
                item.repo.clone(),
                item.kind.clone(),
                item.number,
                body,
            ),
            "github" => github::git_github_work_item_comment_for(
                self.project(item),
                &item.repo,
                &item.kind,
                item.number,
                &body,
                &reply_to,
            ),
            _ => Err("Unsupported connector".into()),
        }
    }
    pub fn diff(&self, item: &Item) -> Result<Diff, String> {
        match item.provider.as_str() {
            "gitlab" => convert(gitlab::gitlab_mr_diff(
                self.data_dir.clone(),
                item.repo.clone(),
                item.number,
            )?),
            "github" => convert(github::git_github_pr_diff_for(
                self.project(item),
                &item.repo,
                item.number,
            )?),
            _ => Err("This issue has no pull request diff".into()),
        }
    }
    pub fn checks(&self, item: &Item) -> Result<Checks, String> {
        if item.provider != "github" {
            return Err("This action requires a GitHub item".into());
        }
        convert(github::git_github_pr_checks_for(
            self.project(item),
            &item.repo,
            item.number,
        )?)
    }
    pub fn action(&self, item: &Item, action: &str) -> Result<Item, String> {
        if item.provider != "github" {
            return Err("This action requires a GitHub item".into());
        }
        let mut updated: Item = convert(github::git_github_pr_action_for(
            self.project(item),
            &item.repo,
            item.number,
            action,
        )?)?;
        updated.provider = item.provider.clone();
        updated.project_path = item.project_path.clone();
        Ok(updated)
    }
}
