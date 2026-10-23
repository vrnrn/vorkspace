//! Validate an app-requested launch before allocating a session or worktree.
use crate::{
    agent::{AccessMode, Settings},
    provider::Kind,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Launch {
    pub provider: Kind,
    pub settings: Settings,
    pub prompt: String,
    pub title: String,
    pub draft: bool,
    pub reveal: bool,
    pub created_worktree: Option<CreatedWorktree>,
    workspace: Workspace,
}
/// Proof of the resource this one launch created; existing checkouts never
/// acquire this cleanup capability. Removal refuses dirty or changed work.
pub struct CreatedWorktree {
    project: String,
    path: String,
    branch: String,
    head: String,
}
impl CreatedWorktree {
    fn capture(project: &str, path: &str) -> Result<Self, String> {
        let snapshot = crate::git::snapshot(Path::new(project))?;
        let tree = snapshot
            .worktrees
            .iter()
            .find(|tree| crate::files::path_key(&tree.path) == crate::files::path_key(path))
            .ok_or(
                "Created worktree could not be verified; inspect working copies before retrying",
            )?;
        Ok(Self {
            project: project.into(),
            path: tree.path.clone(),
            branch: tree
                .branch
                .clone()
                .ok_or("Created worktree has no branch; it was preserved")?,
            head: tree.head.clone(),
        })
    }
    pub fn rollback(self) -> Result<(), String> {
        let preserved =
            |error: String| format!("Created worktree {} was preserved: {error}", self.path);
        if crate::files::remote::is_remote(Path::new(&self.project)) {
            return Err(preserved(
                "the host has no safe removal operation; inspect it before requesting a new launch"
                    .into(),
            ));
        }
        let snapshot = crate::git::snapshot(Path::new(&self.project)).map_err(preserved)?;
        let tree = snapshot
            .worktrees
            .iter()
            .find(|tree| crate::files::path_key(&tree.path) == crate::files::path_key(&self.path))
            .ok_or_else(|| preserved("its ownership is unavailable".into()))?;
        if tree.main || tree.branch.as_deref() != Some(&self.branch) || tree.head != self.head {
            return Err(preserved("its branch or commit changed".into()));
        }
        crate::git::git(
            Path::new(&self.project),
            &["worktree", "remove", "--", &self.path],
        )
        .map_err(preserved)?;
        crate::git::git(Path::new(&self.project),&["update-ref","-d",&format!("refs/heads/{}",self.branch),&self.head]).map_err(|error|format!("Worktree was removed, but its branch was preserved because ownership changed: {error}"))?;
        Ok(())
    }
}
enum Workspace {
    Current,
    Existing(String),
    New(String),
}
fn text(input: &Value, key: &str, max: usize) -> Result<Option<String>, String> {
    input
        .get(key)
        .map(|value| {
            value
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= max && !s.contains('\0'))
                .map(str::to_owned)
                .ok_or_else(|| format!("{key} must contain 1–{max} bytes of text"))
        })
        .transpose()
}
impl Launch {
    pub fn parse(
        input: &Value,
        source_provider: &str,
        source_project: &str,
        project: &str,
        mut settings: Settings,
        default_reveal: bool,
    ) -> Result<Self, String> {
        super::api::validate("sessions.start", input)?;
        let prompt = text(input, "prompt", 65536)?.ok_or("Provide prompt")?;
        let provider = text(input, "harness", 64)?.unwrap_or_else(|| source_provider.into());
        let provider = Kind::parse(&provider).ok_or("Unknown provider")?;
        let same = provider.id() == source_provider;
        if !same {
            settings.account = Default::default();
            settings.model = None;
            settings.effort = None;
            settings.model_settings.clear();
            settings.fast = false;
            settings.service_tier = None;
            settings.thinking = None;
        }
        settings.bot = None;
        settings.bot_message_id = None;
        settings.bot_sent_at = None;
        settings.internal_message = false;
        settings.environment.clear();
        settings.attachments.clear();
        settings.turns.clear();
        settings.handoff = None;
        settings.pending_provider_switch = None;
        settings.prepared_prompt = None;
        settings.turn_model = None;
        settings.remote_action = None;
        settings.usage_limit = None;
        settings.operator = false;
        settings.orchestrator = false;
        settings.plan = false;
        settings.new_worktree = false;
        settings.worktree_base = None;
        settings.worktree_removed = false;
        settings.revision = 0;
        if crate::files::path_key(project) != crate::files::path_key(source_project) {
            settings.worktree_cwd = None;
        }
        if let Some(model) = text(input, "model", 512)? {
            if model
                .split_once(':')
                .is_some_and(|(prefix, _)| Kind::parse(prefix).is_some_and(|kind| kind != provider))
            {
                return Err("Model belongs to another provider".into());
            }
            settings.model =
                Some(crate::store::native_model(&model, provider.id()).ok_or("Invalid model")?);
        }
        if let Some(values) = input.get("modelSettings") {
            let values = values
                .as_object()
                .filter(|v| v.len() <= 64)
                .ok_or("modelSettings must be an object of up to 64 strings")?;
            let mut parsed = std::collections::HashMap::new();
            for (key, value) in values {
                if key.is_empty() || key.len() > 128 || key.contains('\0') {
                    return Err("Invalid modelSettings key".into());
                }
                let value = value
                    .as_str()
                    .filter(|v| v.len() <= 4096 && !v.contains('\0'))
                    .ok_or("modelSettings must contain bounded strings")?;
                parsed.insert(key.clone(), value.to_owned());
            }
            settings.model_settings = parsed;
            // Common launch controls have dedicated transport fields too.
            if let Some(effort) = settings
                .model_settings
                .get("effort")
                .or_else(|| settings.model_settings.get("reasoningEffort"))
            {
                settings.effort = Some(effort.clone());
            }
            if let Some(fast) = settings.model_settings.get("fastMode") {
                settings.fast = match fast.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => return Err("fastMode must be true or false".into()),
                };
            }
            if let Some(thinking) = settings.model_settings.get("thinking") {
                settings.thinking = Some(match thinking.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => return Err("thinking must be true or false".into()),
                });
            }
            if let Some(tier) = settings.model_settings.get("serviceTier") {
                settings.service_tier = Some(tier.clone());
            }
        }
        if let Some(mode) = text(input, "runtimeMode", 32)? {
            settings.access = match mode.as_str() {
                "read-only" => AccessMode::ReadOnly,
                "supervised" => AccessMode::Supervised,
                "auto-accept-edits" => AccessMode::AutoAcceptEdits,
                "auto" => AccessMode::Auto,
                "full-access" => AccessMode::FullAccess,
                _ => return Err("Unknown runtimeMode".into()),
            };
        }
        let mode = text(input, "workspaceMode", 32)?.unwrap_or_else(|| "current".into());
        let cwd = text(input, "worktreeCwd", 4096)?;
        let base = text(input, "worktreeBase", 512)?;
        if mode == "worktree" && cwd.is_some() {
            return Err("Choose a new worktree or an existing worktreeCwd, not both".into());
        }
        if mode != "worktree" && base.is_some() {
            return Err("worktreeBase requires workspaceMode worktree".into());
        }
        let workspace = if mode == "worktree" {
            Workspace::New(base.unwrap_or_else(|| "HEAD".into()))
        } else if let Some(cwd) = cwd {
            Workspace::Existing(cwd)
        } else {
            Workspace::Current
        };
        let title = text(input, "title", 640)?
            .unwrap_or_else(|| prompt.clone())
            .chars()
            .take(160)
            .collect();
        Ok(Self {
            provider,
            settings,
            prompt,
            title,
            draft: input.get("draft").and_then(Value::as_bool).unwrap_or(false),
            created_worktree: None,
            reveal: input
                .get("reveal")
                .and_then(Value::as_bool)
                .unwrap_or(default_reveal),
            workspace,
        })
    }
    pub fn validate_workspace(&mut self, project: &str) -> Result<(), String> {
        match &self.workspace {
            Workspace::Current => {
                if let Some(cwd) = self.settings.worktree_cwd.clone() {
                    self.settings.worktree_cwd = Some(existing(project, &cwd)?);
                }
            }
            Workspace::Existing(cwd) => self.settings.worktree_cwd = Some(existing(project, cwd)?),
            Workspace::New(base) => {
                let snapshot = crate::git::snapshot(Path::new(project))?;
                if crate::files::remote::is_remote(Path::new(project)) {
                    if base != "HEAD" && !snapshot.branches.iter().any(|b| b.name == *base) {
                        return Err("Choose HEAD or a listed branch for a remote worktree".into());
                    }
                } else {
                    crate::git::git(
                        Path::new(project),
                        &[
                            "rev-parse",
                            "--verify",
                            "--end-of-options",
                            &format!("{base}^{{commit}}"),
                        ],
                    )?;
                }
                self.settings.worktree_cwd = None;
            }
        }
        Ok(())
    }
    pub fn create(
        &mut self,
        store: &mut crate::store::Store,
        project: &str,
    ) -> Result<crate::store::Session, String> {
        self.create_configured(store, project, None)
    }
    /// Keep preparation under one owner until its parent link is durable. A
    /// failed link cannot escape with resources no UI installation will own.
    pub fn create_started(
        &mut self,
        store: &mut crate::store::Store,
        project: &str,
        caller: i64,
    ) -> Result<crate::store::Session, String> {
        self.create_configured(store, project, Some(caller))
    }
    fn create_configured(
        &mut self,
        store: &mut crate::store::Store,
        project: &str,
        caller: Option<i64>,
    ) -> Result<crate::store::Session, String> {
        self.validate_workspace(project)?;
        let mut summary = store.create(self.provider.id(), project)?;
        let configured = (|| {
            self.create_workspace(project, summary.id)?;
            if matches!(self.workspace, Workspace::New(_)) {
                self.created_worktree = Some(CreatedWorktree::capture(
                    project,
                    self.settings
                        .worktree_cwd
                        .as_deref()
                        .ok_or("Created worktree path is unavailable")?,
                )?);
            }
            store.title(summary.id, &self.title)?;
            store.configure_agent(summary.id, self.provider.id(), &self.settings)?;
            if let Some(caller) = caller {
                store.save_started_link(summary.id, caller)?;
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = configured {
            // Remove durable ownership before its working copy. If deletion
            // fails, preserve both and report the retained resource explicitly.
            let cleanup = match store.delete(summary.id) {
                Ok(()) => self.created_worktree.take().map(CreatedWorktree::rollback).transpose().err(),
                Err(error) => Some(format!(
                    "Unused session {} could not be removed; its working copy{} was preserved: {error}",
                    summary.id,
                    self.created_worktree.as_ref().map(|tree|format!(" {}",tree.path)).unwrap_or_default(),
                )),
            };
            return Err(format!(
                "Launch was not accepted: {error}{}",
                cleanup
                    .map(|error| format!(". {error}"))
                    .unwrap_or_default(),
            ));
        }
        summary.title = self.title.clone();
        Ok(summary)
    }
    fn create_workspace(&mut self, project: &str, session: i64) -> Result<(), String> {
        if let Workspace::New(base) = &self.workspace {
            self.settings.worktree_cwd = Some(
                crate::git::create_worktree(Path::new(project), base, session)?
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        Ok(())
    }
}
fn existing(project: &str, cwd: &str) -> Result<String, String> {
    let snapshot = crate::git::snapshot(Path::new(project))?;
    let tree = snapshot
        .worktrees
        .iter()
        .find(|tree| {
            !tree.missing && crate::files::path_key(&tree.path) == crate::files::path_key(cwd)
        })
        .ok_or("Choose an existing worktree of the selected project")?;
    if !crate::files::remote::is_remote(Path::new(&tree.path))
        && !PathBuf::from(&tree.path).is_dir()
    {
        return Err("The selected worktree is unavailable".into());
    }
    Ok(tree.path.clone())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn settings() -> Settings {
        Settings {
            access: AccessMode::ReadOnly,
            effort: Some("low".into()),
            worktree_cwd: Some("/other-project".into()),
            operator: true,
            ..Default::default()
        }
    }
    #[test]
    fn explicit_launch_settings_are_preserved_and_ephemeral_capabilities_are_not_inherited() {
        let launch=Launch::parse(&json!({"prompt":"hello","runtimeMode":"supervised","modelSettings":{"effort":"high","custom":"kept"},"workspaceMode":"worktree"}),"codex","/old","/project",settings(),true).unwrap();
        assert_eq!(launch.settings.access, AccessMode::Supervised);
        assert_eq!(launch.settings.effort.as_deref(), Some("high"));
        assert_eq!(launch.settings.model_settings["custom"], "kept");
        assert!(!launch.settings.operator);
        assert!(launch.settings.worktree_cwd.is_none());
        assert!(matches!(launch.workspace, Workspace::New(_)));
        let inherited = Launch::parse(
            &json!({"prompt":"x"}),
            "codex",
            "/project",
            "/project",
            settings(),
            false,
        )
        .unwrap();
        assert_eq!(inherited.settings.access, AccessMode::ReadOnly);
    }
    #[test]
    fn malformed_or_ambiguous_settings_are_rejected_before_any_creation() {
        for input in [
            json!({"runtimeMode":"unsafe"}),
            json!({"workspaceMode":"unsafe"}),
            json!({"modelSettings":[]}),
            json!({"modelSettings":{"effort":4}}),
            json!({"workspaceMode":"worktree","worktreeCwd":"/outside"}),
            json!({"worktreeBase":"HEAD"}),
            json!({"placement":"wrong"}),
            json!({"besideSessionId":4}),
        ] {
            let mut input = input;
            input["prompt"] = json!("hello");
            assert!(
                Launch::parse(
                    &input,
                    "codex",
                    "/project",
                    "/project",
                    Settings::default(),
                    false
                )
                .is_err(),
                "{input}"
            );
        }
    }
    #[test]
    fn outside_worktree_and_invalid_base_fail_without_allocating_sessions_or_directories() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .current_dir(&project)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ]);
        for input in [
            json!({"prompt":"x","worktreeCwd":directory.path().to_string_lossy()}),
            json!({"prompt":"x","workspaceMode":"worktree","worktreeBase":"missing-ref"}),
        ] {
            let mut launch = Launch::parse(
                &input,
                "codex",
                project.to_str().unwrap(),
                project.to_str().unwrap(),
                Settings::default(),
                false,
            )
            .unwrap();
            assert!(launch
                .validate_workspace(project.to_str().unwrap())
                .is_err());
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }
    }
    #[test]
    fn failed_configuration_removes_its_allocated_session() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("db");
        let mut store = crate::store::Store::open_vorkspace(&path).unwrap();
        // Session creation succeeds; title publication fails after allocation.
        rusqlite::Connection::open(&path).unwrap().execute_batch("CREATE TRIGGER reject_title BEFORE UPDATE OF title ON sessions BEGIN SELECT RAISE(ABORT,'title failure'); END").unwrap();
        let mut launch = Launch::parse(
            &json!({"prompt":"hello"}),
            "codex",
            "/project",
            "/project",
            Settings::default(),
            false,
        )
        .unwrap();
        assert!(launch
            .create(&mut store, "/project")
            .unwrap_err()
            .contains("title failure"));
        assert!(store.sessions().unwrap().is_empty());
    }
    #[test]
    fn started_launch_persists_parent_link_in_both_schemas() {
        for shared in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("state.db");
            let open = || {
                if shared {
                    crate::store::Store::open_vorkspace(&path)
                } else {
                    crate::store::Store::open(&path)
                }
            };
            let mut store = open().unwrap();
            let caller = store.create("codex", "/project").unwrap().id;
            let mut launch = Launch::parse(
                &json!({"prompt":"Child request"}),
                "codex",
                "/project",
                "/project",
                Settings::default(),
                true,
            )
            .unwrap();
            let summary = launch
                .create_started(&mut store, "/project", caller)
                .unwrap();
            assert_eq!(
                store.started_links().unwrap().get(&summary.id),
                Some(&caller)
            );
            drop(store);
            let reopened = open().unwrap();
            assert_eq!(
                reopened.started_links().unwrap().get(&summary.id),
                Some(&caller)
            );
            assert_eq!(reopened.sessions().unwrap().len(), 2);
        }
    }
    #[test]
    fn failed_configuration_keeps_worktree_if_session_deletion_fails() {
        for shared in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let project = root.path().join("project");
            std::fs::create_dir(&project).unwrap();
            for args in [
                vec!["init", "-q"],
                vec![
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "initial",
                ],
            ] {
                assert!(std::process::Command::new("git")
                    .current_dir(&project)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success());
            }
            let path = root.path().join("state.db");
            let mut store = if shared {
                crate::store::Store::open_vorkspace(&path)
            } else {
                crate::store::Store::open(&path)
            }
            .unwrap();
            let table = if shared {
                "sessions"
            } else {
                "native_sessions"
            };
            rusqlite::Connection::open(&path).unwrap().execute_batch(&format!("CREATE TRIGGER reject_title BEFORE UPDATE OF title ON {table} BEGIN SELECT RAISE(ABORT,'injected title failure'); END;
                CREATE TRIGGER reject_cleanup BEFORE DELETE ON {table} BEGIN SELECT RAISE(ABORT,'injected cleanup failure'); END;")).unwrap();
            let project_text = project.to_str().unwrap();
            let mut launch = Launch::parse(
                &json!({"prompt":"Child request","workspaceMode":"worktree"}),
                "codex",
                project_text,
                project_text,
                Settings::default(),
                true,
            )
            .unwrap();
            let error = launch
                .create_started(&mut store, project_text, 1)
                .unwrap_err();
            assert!(error.contains("injected title failure"), "{error}");
            assert!(error.contains("injected cleanup failure"), "{error}");
            assert!(error.contains("preserved"), "{error}");
            let tree = launch.settings.worktree_cwd.as_ref().unwrap();
            assert!(error.contains(tree), "{error}");
            assert!(Path::new(tree).is_dir());
            assert_eq!(store.sessions().unwrap().len(), 1);
            assert_eq!(crate::git::snapshot(&project).unwrap().worktrees.len(), 2);
        }
    }
    #[test]
    fn new_worktree_is_rolled_back_after_a_later_database_failure() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .current_dir(&project)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        }
        let path = root.path().join("db");
        let mut store = crate::store::Store::open_vorkspace(&path).unwrap();
        rusqlite::Connection::open(&path).unwrap().execute_batch("CREATE TRIGGER reject_title BEFORE UPDATE OF title ON sessions BEGIN SELECT RAISE(ABORT,'title failure'); END").unwrap();
        let mut launch = Launch::parse(
            &json!({"prompt":"hello","workspaceMode":"worktree"}),
            "codex",
            project.to_str().unwrap(),
            project.to_str().unwrap(),
            Settings::default(),
            false,
        )
        .unwrap();
        let error = launch
            .create(&mut store, project.to_str().unwrap())
            .unwrap_err();
        assert!(error.contains("title failure"), "{error}");
        assert!(!error.contains("preserved"), "{error}");
        assert!(store.sessions().unwrap().is_empty());
        let snapshot = crate::git::snapshot(&project).unwrap();
        assert_eq!(snapshot.worktrees.len(), 1);
        assert!(!snapshot
            .branches
            .iter()
            .any(|branch| branch.name.starts_with("mc/native-")));
    }
    #[test]
    fn rollback_preserves_concurrent_work_in_its_created_checkout() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .current_dir(&project)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        }
        let mut store = crate::store::Store::open_vorkspace(&root.path().join("db")).unwrap();
        let mut launch = Launch::parse(
            &json!({"prompt":"hello","workspaceMode":"worktree"}),
            "codex",
            project.to_str().unwrap(),
            project.to_str().unwrap(),
            Settings::default(),
            false,
        )
        .unwrap();
        launch
            .create(&mut store, project.to_str().unwrap())
            .unwrap();
        let path = Path::new(launch.settings.worktree_cwd.as_deref().unwrap());
        std::fs::write(path.join("user-work.txt"), "preserve me").unwrap();
        let error = launch
            .created_worktree
            .take()
            .unwrap()
            .rollback()
            .unwrap_err();
        assert!(error.contains("preserved"));
        assert_eq!(
            std::fs::read_to_string(path.join("user-work.txt")).unwrap(),
            "preserve me"
        );
        assert_eq!(crate::git::snapshot(&project).unwrap().worktrees.len(), 2);
    }
}
