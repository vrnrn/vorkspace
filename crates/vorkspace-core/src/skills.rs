//! Skill discovery ported from the retired Tauri backend. No UI or transport dependencies.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::scm::shell::dirs_home;
fn expand_home(path: &str) -> PathBuf {
    crate::scm::expand_home(Path::new(path))
}

const MAX_SKILLS: usize = 5_000;
const MAX_FRONTMATTER_BYTES: usize = 16 * 1024;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredSkill {
    pub name: String,
    pub description: String,
    pub path: String,
    pub scope: String,
    pub source: String,
}

struct DisabledFilter {
    normalized: HashSet<String>,
    canonical: HashSet<PathBuf>,
}

impl DisabledFilter {
    fn new(paths: Option<&[String]>) -> Option<Self> {
        let paths = paths?;
        if paths.is_empty() {
            return None;
        }
        let normalized = paths
            .iter()
            .map(|p| normalize_path_for_compare(p))
            .collect();
        let canonical = paths
            .iter()
            .filter_map(|p| std::fs::canonicalize(expand_home(p)).ok())
            .collect();
        Some(Self {
            normalized,
            canonical,
        })
    }

    fn is_disabled(&self, path: &str) -> bool {
        let normalized = normalize_path_for_compare(path);
        if self.normalized.contains(&normalized) {
            return true;
        }
        if !self.canonical.is_empty() {
            if let Ok(canon) = std::fs::canonicalize(path) {
                if self.canonical.contains(&canon) {
                    return true;
                }
            }
        }
        false
    }
}

fn normalize_path_for_compare(path: &str) -> String {
    path.to_string()
}

/// Skills visible for the open project: `.agents/skills` first, then native
/// harness folders. Same name: earlier roots win.
/// Excludes disabled paths before deduplication so lower-priority enabled
/// same-name files can fall through.
pub fn list_skills(
    cwd: String,
    disabled_paths: Option<Vec<String>>,
) -> Result<Vec<DiscoveredSkill>, String> {
    let project = expand_home(&cwd);
    let home = dirs_home().map(PathBuf::from);
    let mut skills = list_skills_from(&project, home.as_deref(), disabled_paths.as_deref());
    let disabled = DisabledFilter::new(disabled_paths.as_deref());
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".codex")));
    if let Some(codex_home) = codex_home {
        let mut seen: HashSet<_> = skills.iter().map(|s| s.name.clone()).collect();
        for root in [codex_home.join("skills"), codex_home.join("skills/.system")] {
            for skill in scan_root(&root, "user", "codex") {
                if skills.len() >= MAX_SKILLS {
                    break;
                }
                if !disabled
                    .as_ref()
                    .is_some_and(|f| f.is_disabled(&skill.path))
                    && seen.insert(skill.name.clone())
                {
                    skills.push(skill);
                }
            }
        }
        skills.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Ok(skills)
}

pub(crate) fn list_skills_from(
    project: &Path,
    home: Option<&Path>,
    disabled_paths: Option<&[String]>,
) -> Vec<DiscoveredSkill> {
    let disabled_filter = DisabledFilter::new(disabled_paths);
    let mut by_name: HashMap<String, DiscoveredSkill> = HashMap::new();
    let mut seen_roots: HashSet<PathBuf> = HashSet::new();

    let mut add_root = |root: PathBuf, scope: &str, source: &str| {
        if by_name.len() >= MAX_SKILLS {
            return;
        }
        let key = std::fs::canonicalize(&root).unwrap_or(root.clone());
        if !seen_roots.insert(key) {
            return;
        }
        for skill in scan_root(&root, scope, source) {
            if disabled_filter
                .as_ref()
                .is_some_and(|f| f.is_disabled(&skill.path))
            {
                continue;
            }
            if by_name.len() >= MAX_SKILLS {
                break;
            }
            by_name.entry(skill.name.clone()).or_insert(skill);
        }
    };

    // Highest priority first so later roots cannot replace a name.
    add_root(project.join(".agents/skills"), "project", "agents");
    if let Some(home) = home {
        add_root(home.join(".agents/skills"), "user", "agents");
    }

    for (dir, source) in [
        (".claude/skills", "claude"),
        (".codex/skills", "codex"),
        (".pi/skills", "pi"),
        (".omp/skills", "omp"),
    ] {
        add_root(project.join(dir), "project", source);
        if let Some(home) = home {
            add_root(home.join(dir), "user", source);
        }
    }
    if let Some(home) = home {
        add_root(home.join(".pi/agent/skills"), "user", "pi");
        add_root(home.join(".omp/agent/skills"), "user", "omp");
        // New-provider roots come after every pre-existing root so an
        // identically named skill can never shadow an established provider.
        let root = home.join(".gemini/antigravity/skills");
        if root.is_dir() {
            add_root(root, "user", "antigravity");
        }
        for (root, scope, namespace) in claude_plugin_skill_roots(home, project) {
            add_namespaced_root(
                &mut by_name,
                root,
                scope,
                "claude",
                &namespace,
                disabled_filter.as_ref(),
            );
        }
    }

    let mut out: Vec<DiscoveredSkill> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn add_namespaced_root(
    by_name: &mut HashMap<String, DiscoveredSkill>,
    root: PathBuf,
    scope: &str,
    source: &str,
    namespace: &str,
    disabled_filter: Option<&DisabledFilter>,
) {
    if by_name.len() >= MAX_SKILLS {
        return;
    }
    for mut skill in scan_root(&root, scope, source) {
        if disabled_filter.is_some_and(|f| f.is_disabled(&skill.path)) {
            continue;
        }
        if by_name.len() >= MAX_SKILLS {
            break;
        }
        skill.name = format!("{namespace}:{}", skill.name);
        by_name.entry(skill.name.clone()).or_insert(skill);
    }
}

fn claude_plugin_skill_roots(home: &Path, project: &Path) -> Vec<(PathBuf, &'static str, String)> {
    let registry = home.join(".claude/plugins/installed_plugins.json");
    let Ok(raw) = std::fs::read_to_string(registry) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(plugins) = value.get("plugins").and_then(|value| value.as_object()) else {
        return Vec::new();
    };

    let mut roots = Vec::new();
    for (plugin_id, installed) in plugins {
        if !claude_plugin_enabled(home, project, plugin_id) {
            continue;
        }
        let namespace = plugin_id
            .rsplit_once('@')
            .map(|(name, _)| name)
            .unwrap_or(plugin_id);
        if !is_valid_skill_name(namespace) {
            continue;
        }
        let entries: Vec<&serde_json::Value> = match installed.as_array() {
            Some(entries) => entries.iter().collect(),
            None if installed.is_object() => vec![installed],
            None => continue,
        };
        for entry in entries {
            let Some(install_path) = entry.get("installPath").and_then(|value| value.as_str())
            else {
                continue;
            };
            let scope = match entry.get("scope").and_then(|value| value.as_str()) {
                Some("project" | "local") => {
                    let Some(project_path) =
                        entry.get("projectPath").and_then(|value| value.as_str())
                    else {
                        continue;
                    };
                    if !path_is_within(project, &resolve_home_path(project_path, home)) {
                        continue;
                    }
                    "project"
                }
                Some("user") | None => "user",
                Some(_) => continue,
            };
            roots.push((
                resolve_home_path(install_path, home).join("skills"),
                scope,
                namespace.to_string(),
            ));
        }
    }
    roots.sort_by_key(|(_, scope, _)| if *scope == "project" { 0 } else { 1 });
    roots
}

fn resolve_home_path(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        home.join(path)
    }
}

fn claude_plugin_enabled(home: &Path, project: &Path, plugin_id: &str) -> bool {
    if let Some(enabled) = managed_plugin_setting(plugin_id) {
        return enabled;
    }
    let project_root = claude_settings_project_root(project);
    for settings in [
        project_root.join(".claude/settings.local.json"),
        project_root.join(".claude/settings.json"),
        home.join(".claude/settings.json"),
    ] {
        if let Some(enabled) = plugin_setting(&settings, plugin_id) {
            return enabled;
        }
    }
    true
}

fn claude_settings_project_root(project: &Path) -> PathBuf {
    for candidate in project.ancestors() {
        let claude = candidate.join(".claude");
        if claude.join("settings.local.json").is_file() || claude.join("settings.json").is_file() {
            return candidate.to_path_buf();
        }
    }
    project.to_path_buf()
}

fn plugin_setting(path: &Path, plugin_id: &str) -> Option<bool> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    value.get("enabledPlugins")?.get(plugin_id)?.as_bool()
}

fn managed_plugin_setting(plugin_id: &str) -> Option<bool> {
    let root = managed_settings_root()?;
    managed_plugin_setting_from_root(&root, plugin_id)
}

fn managed_plugin_setting_from_root(root: &Path, plugin_id: &str) -> Option<bool> {
    let mut value = plugin_setting(&root.join("managed-settings.json"), plugin_id);
    let dir = root.join("managed-settings.d");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("json")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| !name.starts_with('.'))
        })
        .collect();
    files.sort();
    for file in files {
        if let Some(enabled) = plugin_setting(&file, plugin_id) {
            value = Some(enabled);
        }
    }
    value
}

fn managed_settings_root() -> Option<PathBuf> {
    return Some(PathBuf::from("/Library/Application Support/ClaudeCode"));
    #[allow(unreachable_code)]
    None
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let Ok(path) = std::fs::canonicalize(path) else {
        return false;
    };
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    path == root || path.starts_with(root)
}

fn scan_root(root: &Path, scope: &str, source: &str) -> Vec<DiscoveredSkill> {
    let Ok(reader) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ent in reader.flatten() {
        let dir = ent.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(folder) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if folder.starts_with('.') || folder == "skills-cursor" {
            continue;
        }
        let skill_md = skill_md_path(&dir);
        let Some(skill_md) = skill_md else { continue };
        let Ok(bytes) = read_prefix(&skill_md, MAX_FRONTMATTER_BYTES) else {
            continue;
        };
        let Some(text) = utf8_prefix(bytes) else {
            continue;
        };
        let fallback = slug_name(folder);
        if fallback.is_empty() {
            continue;
        }
        let (name, description) = parse_frontmatter(&text, &fallback);
        if name.is_empty() {
            continue;
        }
        out.push(DiscoveredSkill {
            name,
            description,
            path: crate::scm::path_to_js(&skill_md),
            scope: scope.to_string(),
            source: source.to_string(),
        });
    }
    out
}

fn skill_md_path(dir: &Path) -> Option<PathBuf> {
    let upper = dir.join("SKILL.md");
    if upper.is_file() {
        return Some(upper);
    }
    let lower = dir.join("skill.md");
    if lower.is_file() {
        return Some(lower);
    }
    None
}

fn read_prefix(path: &Path, max: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(max.min(4096));
    file.take(max as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Decodes the bytes from `read_prefix`, which can stop in the middle of a
/// multi-byte character. Removes an incomplete character at the end.
/// Rejects other invalid UTF-8.
fn utf8_prefix(bytes: Vec<u8>) -> Option<String> {
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(err) if err.utf8_error().error_len().is_none() => {
            let valid = err.utf8_error().valid_up_to();
            let mut bytes = err.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).ok()
        }
        Err(_) => None,
    }
}

fn parse_frontmatter(text: &str, fallback: &str) -> (String, String) {
    let trimmed = text.trim_start_matches('\u{feff}');
    let Some(rest) = trimmed.strip_prefix("---") else {
        return (fallback.to_string(), String::new());
    };
    let rest = rest.strip_prefix('\r').unwrap_or(rest);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end = rest
        .find("\n---")
        .or_else(|| rest.find("\r\n---"))
        .unwrap_or(rest.len());
    let yaml = &rest[..end];

    let mut name: Option<String> = None;
    let mut description = String::new();
    let mut in_desc = false;
    let mut fold_desc = false;

    for raw in yaml.lines() {
        if in_desc {
            if is_yaml_indent(raw) {
                let piece = raw.trim();
                if piece.is_empty() {
                    continue;
                }
                if !description.is_empty() {
                    description.push(if fold_desc { ' ' } else { '\n' });
                }
                description.push_str(piece);
                continue;
            }
            in_desc = false;
        }

        let line = raw.trim_end();
        if let Some(value) = yaml_value(line, "name") {
            name = Some(unquote(&value));
        } else if let Some(value) = yaml_value(line, "description") {
            let value = value.trim();
            if is_folded_scalar(value) {
                in_desc = true;
                fold_desc = value.starts_with('>');
                description.clear();
            } else {
                description = unquote(value);
            }
        }
    }

    let folder = fallback.to_string();
    let name = name.filter(|n| is_valid_skill_name(n)).unwrap_or(folder);
    (name, description.trim().to_string())
}

fn yaml_value(line: &str, key: &str) -> Option<String> {
    let line = line.trim_start();
    let prefix = format!("{key}:");
    line.strip_prefix(&prefix)
        .map(|rest| rest.trim().to_string())
}

fn is_folded_scalar(value: &str) -> bool {
    value.starts_with('>') || value.starts_with('|')
}

fn is_yaml_indent(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

pub fn is_valid_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let mut prev_dash = true;
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            prev_dash = false;
            continue;
        }
        if ch == '-' && i > 0 && !prev_dash {
            prev_dash = true;
            continue;
        }
        return false;
    }
    !prev_dash
}

pub fn slug_name(raw: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > 64 {
        out.truncate(64);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

/// Create the same starter SKILL.md as React. Existing skills are never overwritten.
pub fn create_blank(cwd: &str, raw: &str, personal: bool) -> Result<PathBuf, String> {
    let name = slug_name(raw)
        .chars()
        .take(64)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned();
    if !is_valid_skill_name(&name) {
        return Err("Use a lowercase name with letters, numbers, and hyphens.".into());
    }
    let root = if personal || cwd.is_empty() || cwd == "~" {
        PathBuf::from(dirs_home().ok_or("Home directory not found")?)
    } else {
        expand_home(cwd)
    };
    let folder = root.join(".agents/skills").join(&name);
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join("SKILL.md");
    let title = name
        .split('-')
        .map(|s| {
            let mut chars = s.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let body = format!("---\nname: {name}\ndescription: {title}. Use when the user asks to {}.\n---\n\n# {title}\n\n## Instructions\n\n",name.replace('-'," "));
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("Could not create skill: {e}"))?;
    file.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests;
