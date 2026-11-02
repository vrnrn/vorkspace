//! Persistent Bot identities and app-owned context, shared by native views,
//! scheduled runs and the local app CLI. React's bots/model is the reference.
pub mod api;
pub mod bridge;
pub mod cards;
pub mod channels;
pub mod files;
pub mod habits;
pub mod launch;
pub mod memory;
pub mod peers;
mod teams;
#[cfg(test)]
mod tests;
pub mod work;

pub use crate::bot_model::*;
use crate::{
    project_appearance::{
        default_foreground_color, default_seed_for, normalize_badge_shape, normalize_bot_face,
        BOT_FACES, MASCOTS,
    },
    transcript::Row,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

impl Bot {
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.clone();
        }
        let name = if self.mascot == "mushroom" {
            "Shroom".into()
        } else {
            let mut chars = self.mascot.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        };
        format!("Bot{name}")
    }
    pub fn face_style(&self) -> &'static str {
        normalize_bot_face(&self.mascot)
    }
    pub fn badge_shape(&self) -> &'static str {
        normalize_badge_shape(&self.shape)
    }
    pub fn resolved_fg_color(&self) -> u32 {
        self.fg_color
            .filter(|c| *c <= 0xffffff)
            .unwrap_or_else(|| default_foreground_color(self.color))
    }
    pub fn resolved_seed(&self) -> u32 {
        self.seed
            .filter(|&s| s > 0)
            .unwrap_or_else(|| default_seed_for(&self.id))
    }
    pub fn resolved_grid(&self) -> u8 {
        self.grid.unwrap_or(4).clamp(2, 8)
    }
    pub fn works_on(&self, project: &str) -> bool {
        self.projects
            .iter()
            .any(|p| crate::files::path_key(p) == crate::files::path_key(project))
    }
    pub fn project(&self, requested: Option<&str>) -> Result<String, String> {
        if let Some(requested) = requested {
            if let Some(path) = self
                .projects
                .iter()
                .find(|path| crate::files::path_key(path) == crate::files::path_key(requested))
            {
                return Ok(path.clone());
            }
            let named: Vec<_> = self
                .projects
                .iter()
                .filter(|p| {
                    Path::new(p)
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(requested))
                })
                .collect();
            if named.len() == 1 {
                return Ok(named[0].clone());
            }
            return Err("Choose one of this Bot's assigned projects by full path".into());
        }
        match self.projects.as_slice() {
            [project] => Ok(project.clone()),
            [] => Err("Add a project in this Bot's settings first".into()),
            _ => Err("This Bot has several projects; include project with its path".into()),
        }
    }
    pub fn normalize(&mut self) {
        self.name = self
            .name
            .trim()
            .chars()
            .take(if self.is_minion() { 60 } else { 40 })
            .collect();
        if !MASCOTS.contains(&self.mascot.as_str()) && !BOT_FACES.contains(&self.mascot.as_str()) {
            self.mascot = MASCOTS[0].into();
        }
        self.shape = normalize_badge_shape(&self.shape).into();
        if self.color > 0xffffff {
            self.color = COLORS_BOT[0];
        }
        self.fg_color = self.fg_color.filter(|c| *c <= 0xffffff);
        self.seed = self.seed.filter(|&s| s > 0);
        self.grid = self.grid.map(|g| g.clamp(2, 8));
        let mut seen = HashSet::new();
        self.projects
            .retain(|p| !p.trim().is_empty() && seen.insert(crate::files::path_key(p)));
    }
}
pub type Work = (Vec<Document>, Vec<Launch>);
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Roster {
    pub bots: Vec<Bot>,
    pub hidden_sessions: Vec<i64>,
    /// Per-message delivery intent travels with the durable outbox.
    #[serde(default, skip_serializing_if = "std::collections::HashSet::is_empty")]
    pub held_messages: std::collections::HashSet<String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub message_modes: std::collections::HashMap<String, String>,
    /// Last observed desktop identities, for reconciling later desktop edits
    /// without discarding deliberate native edits or resurrecting deletions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) desktop_source: Option<Vec<Bot>>,
    /// Shared by local snapshots so queued saves compare with the last successful publication.
    #[serde(skip)]
    pub(crate) storage_baseline: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}
// Three-way reconciliation applies only this snapshot's changes to the latest
// durable roster. Identified arrays allow independent Bots and messages to be
// edited concurrently; ambiguous edits are rejected rather than overwritten.
pub(crate) fn merge_roster_value(
    base: Option<&serde_json::Value>,
    local: Option<&serde_json::Value>,
    remote: Option<&serde_json::Value>,
    field: &str,
) -> Result<Option<serde_json::Value>, String> {
    use serde_json::Value;
    if local == base {
        return Ok(remote.cloned());
    }
    if remote == base || local == remote {
        return Ok(local.cloned());
    }
    if let (Some(Value::Object(base)), Some(Value::Object(local)), Some(Value::Object(remote))) =
        (base, local, remote)
    {
        let mut result = remote.clone();
        let keys: std::collections::BTreeSet<_> = base
            .keys()
            .chain(local.keys())
            .chain(remote.keys())
            .collect();
        for key in keys {
            match merge_roster_value(base.get(key), local.get(key), remote.get(key), key)? {
                Some(value) => {
                    result.insert(key.clone(), value);
                }
                None => {
                    result.remove(key);
                }
            }
        }
        return Ok(Some(Value::Object(result)));
    }
    if field == "held_messages" {
        let read = |value: Option<&Value>| -> Result<std::collections::BTreeSet<String>, String> {
            match value {
                None => Ok(Default::default()),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| "Invalid held Bot message identity".into())
                    })
                    .collect(),
                _ => Err("Invalid held Bot messages".into()),
            }
        };
        let b = read(base)?;
        let l = read(local)?;
        let r = read(remote)?;
        let ids: std::collections::BTreeSet<_> = b
            .union(&l)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .union(&r)
            .cloned()
            .collect();
        let merged: Vec<Value> = ids
            .into_iter()
            .filter(|id| {
                if l.contains(id) == b.contains(id) {
                    r.contains(id)
                } else {
                    l.contains(id)
                }
            })
            .map(Value::String)
            .collect();
        return Ok((!merged.is_empty()).then_some(Value::Array(merged)));
    }
    if field == "message_modes" && base.is_none() {
        let empty = Value::Object(Default::default());
        return merge_roster_value(Some(&empty), local, remote, "modes");
    }
    if matches!(field, "bots" | "outbox") {
        if let (Some(Value::Array(base)), Some(Value::Array(local)), Some(Value::Array(remote))) =
            (base, local, remote)
        {
            let ids = |items: &[Value]| -> Result<Vec<String>, String> {
                let mut seen = HashSet::new();
                items
                    .iter()
                    .map(|item| {
                        let id = item
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                            .ok_or("Invalid Bot roster identity")?;
                        if !seen.insert(id.to_owned()) {
                            return Err("Duplicate Bot roster identity".into());
                        }
                        Ok(id.to_owned())
                    })
                    .collect()
            };
            let base_ids = ids(base)?;
            let local_ids = ids(local)?;
            let remote_ids = ids(remote)?;
            let common: HashSet<_> = base_ids
                .iter()
                .filter(|id| local_ids.contains(id) && remote_ids.contains(id))
                .collect();
            let order = |ids: &[String]| {
                ids.iter()
                    .filter(|id| common.contains(id))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            let base_order = order(&base_ids);
            let local_order = order(&local_ids);
            let remote_order = order(&remote_ids);
            if local_order != base_order
                && remote_order != base_order
                && local_order != remote_order
            {
                return Err("Bot roster order changed in another window; your changes were preserved in this window.".into());
            }
            let mut ordered = if local_order != base_order {
                local_ids.clone()
            } else {
                remote_ids.clone()
            };
            for id in local_ids.iter().chain(remote_ids.iter()) {
                if !ordered.contains(id) {
                    ordered.push(id.clone());
                }
            }
            let find = |items: &[Value], id: &str| {
                items
                    .iter()
                    .position(|item| item["id"].as_str() == Some(id))
            };
            let mut result = Vec::new();
            for id in ordered {
                let b = find(base, &id).map(|i| &base[i]);
                let l = find(local, &id).map(|i| &local[i]);
                let r = find(remote, &id).map(|i| &remote[i]);
                if let Some(value) = merge_roster_value(b, l, r, &id)? {
                    result.push(value);
                }
            }
            return Ok(Some(Value::Array(result)));
        }
    }
    Err(format!(
        "Bot roster field {field} changed in another window; your changes were preserved in this window."
    ))
}

// Apply understood changes back onto the original JSON, retaining fields
// written by newer or other clients which the native model does not understand.
pub(crate) fn overlay_roster_value(
    old: Option<&serde_json::Value>,
    new: &serde_json::Value,
    raw: Option<&serde_json::Value>,
    field: &str,
) -> serde_json::Value {
    use serde_json::Value;
    if old == Some(new) {
        return raw.cloned().unwrap_or_else(|| new.clone());
    }
    if let (Some(Value::Object(old)), Value::Object(new)) = (old, new) {
        let mut result = raw.and_then(Value::as_object).cloned().unwrap_or_default();
        for key in old.keys().filter(|key| !new.contains_key(*key)) {
            result.remove(key);
        }
        for (key, value) in new {
            result.insert(
                key.clone(),
                overlay_roster_value(old.get(key), value, raw.and_then(|raw| raw.get(key)), key),
            );
        }
        return Value::Object(result);
    }
    if matches!(field, "bots" | "outbox") {
        if let (Some(Value::Array(old)), Value::Array(new)) = (old, new) {
            let raw = raw.and_then(Value::as_array);
            return Value::Array(
                new.iter()
                    .map(|item| {
                        let id = item.get("id").and_then(Value::as_str);
                        let previous = old
                            .iter()
                            .find(|value| value.get("id").and_then(Value::as_str) == id);
                        let original = raw.and_then(|values| {
                            values
                                .iter()
                                .find(|value| value.get("id").and_then(Value::as_str) == id)
                        });
                        overlay_roster_value(previous, item, original, "item")
                    })
                    .collect(),
            );
        }
    }
    new.clone()
}

impl Roster {
    /// Prepare an unsaved identity without changing the roster or its storage.
    pub fn draft(&self, projects: Vec<String>) -> Bot {
        let mut bot = Bot {
            mascot: MASCOTS
                .iter()
                .find(|name| !self.bots.iter().any(|m| &m.mascot == *name))
                .unwrap_or(&MASCOTS[self.bots.len() % MASCOTS.len()])
                .to_string(),
            color: *COLORS_BOT
                .iter()
                .find(|color| !self.bots.iter().any(|m| m.color == **color))
                .unwrap_or(&COLORS_BOT[self.bots.len() % COLORS_BOT.len()]),
            projects,
            ..Default::default()
        };
        bot.normalize();
        bot
    }
    pub fn create(&mut self, mut bot: Bot) -> Result<&Bot, String> {
        bot.normalize();
        if bot.name.is_empty() {
            return Err("Enter a name for this Bot.".into());
        }
        if bot.id.is_empty()
            || !bot
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("Invalid Bot identity.".into());
        }
        if self.bots.iter().any(|existing| existing.id == bot.id) {
            return Err("This Bot already exists.".into());
        }
        self.bots.push(bot);
        Ok(self.bots.last().unwrap())
    }
    pub fn get(&self, id: &str) -> Option<&Bot> {
        self.bots.iter().find(|m| m.id == id)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Bot> {
        self.bots.iter_mut().find(|m| m.id == id)
    }
    pub fn of_session(&self, id: i64) -> Option<&Bot> {
        self.bots.iter().find(|m| m.session_id == Some(id))
    }
    /// The Bot that spawned this minion session, and the minion.
    pub fn of_minion(&self, id: i64) -> Option<(&Bot, &Minion)> {
        self.bots.iter().find_map(|bot| {
            bot.minions
                .iter()
                .find(|minion| minion.session_id == id)
                .map(|minion| (bot, minion))
        })
    }
    pub fn reorder(&mut self, ids: &[String]) {
        let mut seen = HashSet::new();
        let ordered: Vec<_> = ids
            .iter()
            .filter(|id| seen.insert(id.as_str()))
            .filter_map(|id| self.get(id).cloned())
            .collect();
        let mut next = ordered.iter();
        for bot in &mut self.bots {
            if ordered.iter().any(|m| m.id == bot.id) {
                *bot = next.next().unwrap().clone();
            }
        }
    }
}

pub fn load_files(root: &Path, bot: &Bot) -> Result<files::AgentFiles, String> {
    let owner = bot.storage_id();
    let mut files = files::load(root, owner, None)?;
    if files.soul.is_none() {
        let soul = bot
            .initial_configuration
            .as_ref()
            .and_then(|config| config.soul.as_deref())
            .unwrap_or(DEFAULT_SOUL);
        match files::save(root, owner, None, "SOUL.md", soul, Some(&files.soul_hash)) {
            Ok(hash) => {
                files.soul = Some(soul.into());
                files.soul_hash = hash;
            }
            Err(error) if error == files::CONFLICT => files = files::load(root, owner, None)?,
            Err(error) => return Err(error),
        }
    }
    Ok(files)
}

static MINIONS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The experimental Minions setting, published by the app's preferences.
pub fn set_minions_enabled(enabled: bool) {
    MINIONS.store(enabled, std::sync::atomic::Ordering::Relaxed);
}
pub fn minions_enabled() -> bool {
    MINIONS.load(std::sync::atomic::Ordering::Relaxed)
}

const MINIONS_GUIDE: &str = "Minions are persistent team members with roles and specialties. Inspect minions.list and reuse useful members with bots.send {to,message,expectReply:true}; replies arrive automatically, so do not poll. Hire a missing role with minions.hire {name,role,specialties?}, then delegate through bots.send. A hire does not start a task. Inspect saved history before recovering failed tasks with minions.retry or minions.discard {minion,messageId}; never blindly replay uncertain work. Use minions.fire {minion} to retire an unhelpful member; its history stays saved. Do not create a new member for each task or every mention of Minions. Members share your soul, memory, todos and live project assignments. Their communication is limited to you and active siblings, directly or in team-only channels. Users cannot message them.";

/// A minion's context: its parent's soul and memory, shared with the parent
/// and its other minions.
pub fn minion_context(
    bot: &Bot,
    name: &str,
    project: &str,
    files: &files::AgentFiles,
    executable: &Path,
) -> String {
    let cli = format!(
        "{} app",
        shlex::try_quote(&executable.to_string_lossy()).unwrap_or_default()
    );
    format!(
        "<vorkspace_context>\nVorkspace adds this context, not the user. Act on it without quoting or describing it.\n\nYou are {name}, a minion of the Bot {parent}. {parent} gave you the task in the first message. Work in {project} only. Commit work you create in a worktree, and never force a worktree removal or discard work: leave cleanup to {parent} unless told otherwise. Avoid overlapping edits with other minions, and finish with a concise report of what you did and found; it is delivered to {parent}.\n\nThe app CLI is `{cli} ACTION --json JSON`. Its credentials are already in your environment; never print them. You share {parent}'s memory with {parent} and its other minions: use memory.search before repeating work, and memory.add for findings the others need. todos.* is {parent}'s board. You cannot start sessions, spawn minions, or change {parent}'s soul or habits.\n\n<soul>\n{soul}\n</soul>\n\n<memory>\n{memory}\nTopic notes: {topics}\n</memory>\n</vorkspace_context>",
        parent = bot.label(),
        soul = files.soul.as_deref().unwrap_or_default(),
        memory = memory::within_budget(&files.memory),
        topics = files.topics.join(", "),
    )
}

pub fn context(bot: &Bot, files: &files::AgentFiles, executable: &Path) -> String {
    if bot.minion.is_some() {
        return teams::context(bot, files, executable);
    }
    let cli = format!(
        "{} app",
        shlex::try_quote(&executable.to_string_lossy()).unwrap_or_default()
    );
    let projects = if bot.projects.is_empty() {
        "No projects assigned yet. Ask the user to add projects in your settings before doing project work.".into()
    } else {
        bot.projects
            .iter()
            .map(|p| format!("- {p}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "<vorkspace_context>\nVorkspace adds this context, not the user. Act on it without quoting or describing it.\n\nYou are {}, a persistent Bot in vorkspace. Work only on the projects assigned to you:\n{projects}\n\nThe app CLI is `{cli} ACTION --json JSON`. Its credentials are already in your environment; never print them. Run `{cli} help` for the exact actions and inputs. Use sessions.*, worktrees.* and folders.* with project set to an assigned path. Use sessions.start for substantial independent work, inherit your model unless another is appropriate, and avoid overlapping edits. Give delegated work that edits files its own worktree with workspaceMode worktree, and never start two editing sessions in one worktree. Clean up after yourself: when work is integrated, archive its sessions, check worktrees.list, and call worktrees.remove for your finished worktrees that report removable true. Never force a removal, delete files by hand, or discard work. A worktree that isn't removable may hold uncommitted or unmerged work, so tell the user instead. Once delegation is accepted, tell the user and end your turn. You will receive one completion report after every session in that turn's group stops; then inspect and integrate the results. sessions.send supports notifyOnComplete:true. Never poll delegated work.\n\n<soul>\n{}\n</soul>\nStanding instructions rank below the user's current request. Change your soul only when asked, through soul.read and soul.update with expectedHash; never edit its file directly.\n\n<memory>\n{}\nTopic notes: {}\n</memory>\nKeep useful facts through memory.add/replace/remove, use memory.search before answering about earlier facts, and never save credentials. Topic notes are read with memory.read.\n\nHabits are scheduled work in separate sessions. Suggest them when useful, but add one with habits.add only after agreement. State whether its instructions allow changes. Quiet runs should return NOTHING_TO_REPORT.\n\nSave substantial reports with artifacts.write, kind document or html, title, summary and complete body. Cards appear in the chat and overview. For diagrams in replies and documents use ```mermaid or ```svg blocks (HTML documents may inline <svg> or use <pre class=\"mermaid\">); visual docs has the drawing rules. Use notes.write only for the user's Notes.{}\n\nFor acknowledgements requiring no words, your whole reply may be <reaction>👍</reaction> with one fitting emoji. Questions and requests still get a written answer.\n</vorkspace_context>",
        bot.label(),
        files.soul.as_deref().unwrap_or_default(),
        memory::within_budget(&files.memory),
        files.topics.join(", "),
        if minions_enabled() {
            format!("\n\n{MINIONS_GUIDE}")
        } else {
            String::new()
        }
    )
}

/// A fresh provider session gets a bounded brief; the saved chat stays whole.
pub fn rotation_brief(rows: &[Row]) -> String {
    let mut exchanges: Vec<(String, String)> = Vec::new();
    for row in rows {
        if row.is_prompt() {
            if row.first || exchanges.is_empty() {
                exchanges.push((row.text.clone(), String::new()));
            } else if let Some(last) = exchanges.last_mut() {
                last.0.push_str(&row.text);
            }
        } else if row.role == "assistant" {
            if let Some(last) = exchanges.last_mut() {
                if row.first {
                    last.1 = row.text.clone();
                } else {
                    last.1.push_str(&row.text);
                }
            }
        }
    }
    let split = exchanges.len().saturating_sub(2);
    let clip = |text: &str, n: usize| text.chars().take(n).collect::<String>();
    let earlier = exchanges[..split]
        .iter()
        .rev()
        .take(10)
        .map(|(u, a)| {
            clip(
                &format!(
                    "User: {} → {}",
                    u.split_whitespace().collect::<Vec<_>>().join(" "),
                    a.split_whitespace().collect::<Vec<_>>().join(" ")
                ),
                280,
            )
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    let recent = exchanges[split..]
        .iter()
        .map(|(u, a)| format!("User: {}\nYou: {}", clip(u, 2000), clip(a, 3000)))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "Your provider context was rotated while preserving the full chat. Earlier exchanges:\n{earlier}\n\nRecent exchanges:\n{recent}"
    )
}

/// Reuse the provider-owned rollout directory prepared by the desktop app.
/// Nothing is created or copied when the native UI opens a Bot.
pub(crate) fn codex_home(root: &Path, account: &str) -> Result<Option<PathBuf>, String> {
    crate::accounts::validate_id(account)?;
    let family = root
        .parent()
        .ok_or("Bot storage has no data directory")?
        .join("codex-bots");
    let home = crate::compatibility::bot_owner(&family, account, true);
    Ok(home.is_dir().then_some(home))
}
