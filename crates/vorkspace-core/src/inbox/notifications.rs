//! Project identities match the desktop notification preference catalog.
use super::Item;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationProject {
    pub id: String,
    pub name: String,
    pub detail: String,
    pub kind: String,
    pub paths: Vec<String>,
}
impl Item {
    pub fn notification_project(&self) -> NotificationProject {
        let host = url::Url::parse(&self.url)
            .ok()
            .and_then(|url| {
                url.host_str().map(|host| match url.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host.into(),
                })
            })
            .unwrap_or_else(|| "unknown".into())
            .to_lowercase();
        if self.provider == "jira" {
            let project = if self.team_id.is_empty() {
                self.repo.to_lowercase()
            } else {
                self.team_id.clone()
            };
            return NotificationProject {
                id: format!(
                    "jira:{host}:project:{}",
                    if project.is_empty() {
                        "unknown"
                    } else {
                        &project
                    }
                ),
                name: if self.team_name.is_empty() {
                    if self.repo.is_empty() {
                        "Jira project".into()
                    } else {
                        self.repo.clone()
                    }
                } else {
                    self.team_name.clone()
                },
                detail: format!("Jira · {host}"),
                kind: "jira".into(),
                paths: vec![],
            };
        }
        if !self.project_path.is_empty() && self.project_path != "~" {
            return NotificationProject {
                id: format!("local:{}", crate::files::path_key(&self.project_path)),
                name: if self.repo.is_empty() {
                    std::path::Path::new(&self.project_path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    self.repo.clone()
                },
                detail: self.project_path.clone(),
                kind: "repository".into(),
                paths: vec![self.project_path.clone()],
            };
        }
        NotificationProject {
            id: if host != "unknown" {
                format!("repository:{host}/{}", self.repo.to_lowercase())
            } else {
                format!("repository:{}:{}", self.provider, self.repo.to_lowercase())
            },
            name: self.repo.clone(),
            detail: if host == "unknown" {
                self.provider.clone()
            } else {
                host
            },
            kind: "repository".into(),
            paths: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_identity_keeps_existing_local_remote_and_jira_rules() {
        let item = Item {
            provider: "github".into(),
            repo: "Acme/App".into(),
            project_path: "/repo/".into(),
            url: "https://GitHub.com/Acme/App/issues/7".into(),
            ..Default::default()
        };
        assert_eq!(item.notification_project().id, "local:/repo");
        assert_eq!(
            Item {
                project_path: String::new(),
                ..item
            }
            .notification_project()
            .id,
            "repository:github.com/acme/app"
        );
        let jira = Item {
            provider: "jira".into(),
            repo: "ENG".into(),
            team_id: "10001".into(),
            url: "https://Team.atlassian.net/browse/ENG-42".into(),
            ..Default::default()
        };
        assert_eq!(
            jira.notification_project().id,
            "jira:team.atlassian.net:project:10001"
        );
        assert_eq!(
            Item {
                team_id: String::new(),
                ..jira
            }
            .notification_project()
            .id,
            "jira:team.atlassian.net:project:eng"
        );
    }
}
