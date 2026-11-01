//! Validate an app-requested launch before allocating a session or worktree.
use crate::{
    agent::{AccessMode, Settings},
    kind::Kind,
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
    pub(crate) workspace: Workspace,
}
/// Proof of the resource this one launch created; existing checkouts never
/// acquire this cleanup capability. Removal refuses dirty or changed work.
pub struct CreatedWorktree {
    project: String,
    pub(crate) path: String,
    branch: String,
    head: String,
}
impl CreatedWorktree {
    pub(crate) fn capture(project: &str, path: &str) -> Result<Self, String> {
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
pub(crate) enum Workspace {
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
                Some(crate::session::native_model(&model, provider.id()).ok_or("Invalid model")?);
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
    pub(crate) fn create_workspace(&mut self, project: &str, session: i64) -> Result<(), String> {
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
mod tests;
