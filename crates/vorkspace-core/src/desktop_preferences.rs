//! Read desktop webview preferences without modifying WebKit storage or its schema.
mod session_defaults;
mod workspace_preferences;
mod workspace_snapshot;

#[cfg(any(target_os = "macos", test))]
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

#[derive(Clone, Debug, Default)]
pub struct DesktopPreferences {
    values: BTreeMap<String, String>,
    available: bool,
}

impl DesktopPreferences {
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    #[cfg(test)]
    pub(crate) fn from_values(values: &[(&str, Value)]) -> Self {
        Self {
            values: values
                .iter()
                .map(|(key, value)| ((*key).into(), value.to_string()))
                .collect(),
            available: true,
        }
    }
    /// Only the actual desktop data directory may read the desktop webview.
    /// Isolated development databases and copies never inherit personal settings.
    /// Read committed SQLite/WAL data once when opening a store. The store owns
    /// the verified snapshot so session reads never rescan WebKit directories.
    pub fn load(data_dir: &Path) -> Self {
        let desktop = crate::store::vorkspace_database();
        if data_dir
            .canonicalize()
            .ok()
            .zip(desktop.parent().and_then(|p| p.canonicalize().ok()))
            .is_none_or(|(actual, expected)| actual != expected)
        {
            return Self::default();
        }
        #[cfg(target_os = "macos")]
        if let Some(home) = crate::accounts::home() {
            return Self::load_webkit(
                &home
                    .join("Library/WebKit")
                    .join(
                        desktop
                            .parent()
                            .and_then(Path::file_name)
                            .unwrap_or_default(),
                    )
                    .join("WebsiteData/Default"),
            );
        }
        Self::default()
    }

    pub fn is_available(&self) -> bool {
        self.available
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
    pub fn json(&self, key: &str) -> Option<Value> {
        serde_json::from_str(self.get(key)?).ok()
    }

    #[cfg(any(target_os = "macos", test))]
    fn load_webkit(root: &Path) -> Self {
        // WebKit stores origins under two opaque directory components. Do not
        // merge localhost development origins or Inspector storage into the app.
        let mut paths = Vec::new();
        if let Ok(first) = std::fs::read_dir(root) {
            for directory in first.flatten() {
                if let Ok(second) = std::fs::read_dir(directory.path()) {
                    for origin in second.flatten() {
                        let path = origin.path();
                        if std::fs::read(path.join("origin"))
                            .ok()
                            .is_some_and(|bytes| desktop_origin(&bytes))
                        {
                            let database = path.join("LocalStorage/localstorage.sqlite3");
                            if database.is_file() {
                                paths.push(database);
                            }
                        }
                    }
                }
            }
        }
        paths.sort();
        paths
            .into_iter()
            .find_map(|path| Self::read_database(&path).ok())
            .unwrap_or_default()
    }

    #[cfg(any(target_os = "macos", test))]
    fn read_database(path: &Path) -> Result<Self, String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_millis(150))
            .map_err(|e| e.to_string())?;
        let mut query = conn
            .prepare("SELECT key,value FROM ItemTable")
            .map_err(|e| e.to_string())?;
        let rows = query
            .query_map([], |row| {
                let key: String = row.get(0)?;
                let value = match row.get_ref(1)? {
                    ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok().map(str::to_owned),
                    ValueRef::Blob(bytes) => decode_webkit_value(bytes),
                    _ => None,
                };
                Ok((key, value))
            })
            .map_err(|e| e.to_string())?;
        let mut values = BTreeMap::new();
        for row in rows {
            let (key, value) = row.map_err(|e| e.to_string())?;
            if let Some(value) = value {
                let canonical = crate::compatibility::key(&key);
                if (canonical.starts_with("vorkspace.") || canonical.starts_with("vorkspace:"))
                    && (canonical == key || !values.contains_key(&canonical))
                {
                    values.insert(canonical, value);
                }
            }
        }
        Ok(Self {
            values,
            available: true,
        })
    }

    /// Reconcile current desktop project settings on every workspace read.
    /// Baselines distinguish deliberate native changes from stale sidecar data.
    pub fn apply_workspace(&self, workspace: &mut Value) {
        if !self.available {
            return;
        }
        let Some(object) = workspace.as_object_mut() else {
            return;
        };
        let baseline = object
            .get("desktop_project_preferences")
            .cloned()
            .unwrap_or(Value::Null);
        let mut next = Map::new();
        if let Some(projects) = self.projects() {
            let source = json!(projects);
            let current = object.get("projects").cloned().unwrap_or(json!([]));
            let merged = merge_projects(baseline.get("projects"), &current, &source);
            object.insert("projects".into(), merged.clone());
            next.insert("projects".into(), source);
            // A stale sidecar selection may point at an internal Bot chat.
            if !merged
                .as_array()
                .is_some_and(|paths| paths.iter().any(|p| Some(p) == object.get("project")))
            {
                if let Some(project) = merged.as_array().and_then(|paths| paths.first()) {
                    object.insert("project".into(), project.clone());
                    object.insert("active".into(), Value::Null);
                }
            }
        }
        let source = self.appearances();
        let previous = baseline.get("appearance").and_then(Value::as_object);
        if !source.is_empty() || previous.is_some() {
            let current = object
                .entry("project_appearance")
                .or_insert_with(|| json!({}));
            if !current.is_object() {
                *current = json!({});
            }
            let current = current.as_object_mut().unwrap();
            let projects: HashSet<_> = source
                .keys()
                .chain(previous.into_iter().flat_map(|v| v.keys()))
                .cloned()
                .collect();
            for project in projects {
                let record = current.entry(project.clone()).or_insert_with(|| json!({}));
                if !record.is_object() {
                    *record = json!({});
                }
                let record = record.as_object_mut().unwrap();
                for (field, default) in [
                    ("name", json!("")),
                    ("color", Value::Null),
                    ("mascot", Value::Null),
                    ("logo", Value::Null),
                    ("background", Value::Null),
                ] {
                    let new = source
                        .get(&project)
                        .and_then(|v| v.get(field))
                        .unwrap_or(&default);
                    let old = previous
                        .and_then(|p| p.get(&project))
                        .and_then(|v| v.get(field));
                    if field == "background" && old.is_none() && new.is_null() {
                        continue;
                    }
                    if old.is_some_and(|old| {
                        old == new && record.get(field).is_some_and(|current| current != old)
                    }) {
                        continue;
                    }
                    if let Some(value) = reconcile_value(old, record.get(field), Some(new)) {
                        record.insert(field.into(), value);
                    }
                }
            }
            next.insert("appearance".into(), Value::Object(source));
        }
        let model_controls = json!(match self.get("vorkspace.modelControls") {
            Some("beside") => "beside",
            None if matches!(
                self.get("vorkspace.composerEffortVisible"),
                Some("1" | "true")
            ) =>
                "beside",
            _ => "menu",
        });
        let preferences = object.entry("preferences").or_insert_with(|| json!({}));
        if !preferences.is_object() {
            *preferences = json!({});
        }
        let preferences = preferences.as_object_mut().unwrap();
        if !baseline.get("model_controls").is_some_and(|old| {
            old == &model_controls
                && preferences
                    .get("model_controls")
                    .is_some_and(|current| current != old)
        }) {
            preferences.insert("model_controls".into(), model_controls.clone());
        }
        next.insert("model_controls".into(), model_controls);
        let provider = self
            .json("vorkspace.lastModel")
            .and_then(|last| last["harness"].as_str().map(str::to_owned))
            .filter(|id| crate::provider::Kind::parse(id).is_some());
        if provider.is_some() || baseline.get("default_provider").is_some() {
            reconcile_preference(
                preferences,
                &baseline,
                &mut next,
                "default_provider",
                json!(provider),
            );
        }
        // Convert desktop model IDs before they reach native launch controls.
        let projects = self.json("vorkspace.projectProviderSettings.v1");
        if projects.is_some() || baseline.get("project_providers").is_some() {
            let empty = Map::new();
            let projects = projects
                .as_ref()
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            let providers: Map<String, Value> = projects
                .iter()
                .map(|(path, settings)| {
                    let provider = settings["defaultHarness"]
                        .as_str()
                        .filter(|id| crate::provider::Kind::parse(id).is_some());
                    let mut models = workspace_preferences::native_models(&settings["models"]);
                    if let Some(provider) = provider {
                        if let Some(model) = settings["defaultModel"]
                            .as_str()
                            .and_then(|model| crate::store::native_model(model, provider))
                        {
                            models.entry(provider.to_owned()).or_insert(json!(model));
                        }
                    }
                    (
                        crate::files::path_key(path),
                        json!({
                            "provider":provider,
                            "models":models,
                            "hidden":workspace_preferences::providers(&settings["hidden"]),
                        }),
                    )
                })
                .collect();
            reconcile_preference(
                preferences,
                &baseline,
                &mut next,
                "project_providers",
                Value::Object(providers),
            );
        }
        self.apply_preferences(preferences, &baseline, &mut next);
        self.apply_view_preferences(object, &baseline, &mut next);
        if !next.is_empty() {
            object.insert("desktop_project_preferences".into(), Value::Object(next));
        }
    }

    fn projects(&self) -> Option<Vec<String>> {
        let recents = self.json("vorkspace.recentProjects")?;
        let recents = recents.as_array()?;
        let archived: HashSet<String> = self
            .json("vorkspace.archivedProjects")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v["path"].as_str().map(crate::files::path_key))
            .collect();
        let mut candidates = Vec::new();
        for item in recents {
            if let Some(path) = item["path"].as_str().filter(|p| project_path(p)) {
                let path = normalize_path(path);
                if !archived.contains(&crate::files::path_key(&path)) && !candidates.contains(&path)
                {
                    candidates.push(path);
                }
            }
        }
        let order = self
            .json("vorkspace.projectRailOrder")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let pinned = self
            .json("vorkspace.projectRailPinned")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let pinned: HashSet<String> = pinned
            .iter()
            .filter_map(Value::as_str)
            .map(crate::files::path_key)
            .collect();
        let mut ordered = Vec::new();
        for path in order
            .iter()
            .filter_map(Value::as_str)
            .map(normalize_path)
            .chain(candidates.iter().cloned())
        {
            if candidates.contains(&path) && !ordered.contains(&path) {
                ordered.push(path);
            }
        }
        ordered.sort_by_key(|path| !pinned.contains(&crate::files::path_key(path)));
        Some(ordered)
    }

    fn appearances(&self) -> Map<String, Value> {
        let mut out = Map::new();
        let projects = self.projects().unwrap_or_default();
        for (key, field) in [
            ("labels", "name"),
            ("logos", "logo"),
            ("mascots", "mascot"),
            ("colors", "color"),
            ("custom-colors", "color"),
        ] {
            let Some(values) = self.json(&format!("vorkspace:tab-group:{key}")) else {
                continue;
            };
            let Some(values) = values.as_object() else {
                continue;
            };
            for (path, value) in values {
                let Some(value) = value.as_str() else {
                    continue;
                };
                let value = if key == "colors" {
                    let Some(color) = value
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| crate::project_appearance::COLORS.get(i))
                    else {
                        continue;
                    };
                    json!(color)
                } else if key == "custom-colors" {
                    let Some(color) = value
                        .strip_prefix('#')
                        .filter(|s| s.len() == 6)
                        .and_then(|s| u32::from_str_radix(s, 16).ok())
                    else {
                        continue;
                    };
                    json!(color)
                } else {
                    json!(value)
                };
                let matching: Vec<String> = if path.contains('/') || path.contains('\\') {
                    vec![normalize_path(path)]
                } else {
                    projects
                        .iter()
                        .filter(|p| {
                            Path::new(p)
                                .file_name()
                                .is_some_and(|name| name == path.as_str())
                        })
                        .cloned()
                        .collect()
                };
                for path in matching {
                    out.entry(path)
                        .or_insert_with(|| json!({}))
                        .as_object_mut()
                        .unwrap()
                        .insert(field.into(), value.clone());
                }
            }
        }
        for record in out.values_mut().filter_map(Value::as_object_mut) {
            for (field, default) in [
                ("name", json!("")),
                ("color", Value::Null),
                ("mascot", Value::Null),
                ("logo", Value::Null),
            ] {
                record.entry(field).or_insert(default);
            }
        }
        if let Some(backgrounds) = self.json("vorkspace:project-chat-backgrounds") {
            for (path, settings) in backgrounds.as_object().into_iter().flatten() {
                let Some(path_value) = settings["path"]
                    .as_str()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                else {
                    continue;
                };
                let default_empty = self
                    .number("chatBackgroundEmptyOpacity")
                    .or_else(|| self.number("chatBackgroundOpacity"))
                    .unwrap_or(0.24)
                    .clamp(0.05, 0.65);
                let opacity = |name: &str| {
                    settings[name]
                        .as_f64()
                        .filter(|v| v.is_finite())
                        .map(|v| v.clamp(0.05, 0.65))
                };
                let legacy = opacity("opacity").unwrap_or(default_empty);
                let empty = opacity("emptyOpacity").unwrap_or(legacy);
                let session = opacity("sessionOpacity").unwrap_or_else(|| {
                    if opacity("emptyOpacity").is_some() {
                        empty
                    } else if opacity("opacity").is_some() {
                        legacy
                    } else {
                        self.number("chatBackgroundSessionOpacity")
                            .or_else(|| self.number("chatBackgroundOpacity"))
                            .unwrap_or(0.24)
                            .clamp(0.05, 0.65)
                    }
                });
                let scope = settings["scope"]
                    .as_str()
                    .filter(|s| ["empty", "all"].contains(s))
                    .or_else(|| {
                        self.get("vorkspace.chatBackgroundScope")
                            .filter(|s| ["empty", "all"].contains(s))
                    })
                    .unwrap_or("all");
                let effect = settings["effect"]
                    .as_str()
                    .filter(|s| {
                        [
                            "none",
                            "dither",
                            "ascii",
                            "halftone",
                            "scanlines",
                            "gradient-blur",
                        ]
                        .contains(s)
                    })
                    .unwrap_or("none");
                out.entry(normalize_path(path)).or_insert_with(|| json!({})).as_object_mut().unwrap().insert("background".into(), json!({"path":path_value, "empty_opacity":empty, "session_opacity":session, "scope":scope, "effect":effect}));
            }
        }
        out
    }
}

fn reconcile_preference(
    preferences: &mut Map<String, Value>,
    baseline: &Value,
    next: &mut Map<String, Value>,
    field: &str,
    source: Value,
) {
    let merged = reconcile_value(baseline.get(field), preferences.get(field), Some(&source));
    preferences.insert(field.into(), merged.unwrap_or(Value::Null));
    next.insert(field.into(), source);
}

// Preserve native edits at individual map keys when an unrelated desktop key
// changes. A desktop edit to the same scalar wins on the next store open.
fn reconcile_value(
    old: Option<&Value>,
    current: Option<&Value>,
    source: Option<&Value>,
) -> Option<Value> {
    if old == source && current.is_some() {
        return current.cloned();
    }
    if old.is_none() || current == old || current.is_none() {
        return source.cloned();
    }
    if let (Some(old), Some(current), Some(source)) = (
        old.and_then(Value::as_object),
        current.and_then(Value::as_object),
        source.and_then(Value::as_object),
    ) {
        let keys: HashSet<_> = old
            .keys()
            .chain(current.keys())
            .chain(source.keys())
            .collect();
        return Some(Value::Object(
            keys.into_iter()
                .filter_map(|key| {
                    reconcile_value(old.get(key), current.get(key), source.get(key))
                        .map(|value| (key.clone(), value))
                })
                .collect(),
        ));
    }
    source.cloned()
}

fn normalize_path(path: &str) -> String {
    let path = if path.contains('\\')
        && (path.as_bytes().get(1) == Some(&b':') || path.starts_with("\\\\"))
    {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    };
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.into()
    }
}
fn project_path(path: &str) -> bool {
    let path = normalize_path(path);
    !path.is_empty()
        && path != "/"
        && path != "~"
        && !path.contains(".app/")
        && !(path.len() == 2 && path.ends_with(':'))
        && crate::accounts::home().is_none_or(|home| Path::new(&path) != home)
}
fn merge_projects(old: Option<&Value>, current: &Value, source: &Value) -> Value {
    let Some(old) = old.and_then(Value::as_array) else {
        return source.clone();
    };
    let current = current.as_array().cloned().unwrap_or_default();
    let mut merged: Vec<Value> = source
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|path| !old.contains(path) || current.contains(path))
        .collect();
    for path in current {
        if path.as_str().is_some_and(project_path)
            && !old.contains(&path)
            && !merged.contains(&path)
        {
            merged.push(path);
        }
    }
    Value::Array(merged)
}
#[cfg(any(target_os = "macos", test))]
fn decode_webkit_value(bytes: &[u8]) -> Option<String> {
    let (pairs, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    String::from_utf16(
        &pairs
            .iter()
            .copied()
            .map(u16::from_le_bytes)
            .collect::<Vec<_>>(),
    )
    .ok()
}
#[cfg(any(target_os = "macos", test))]
fn desktop_origin(mut bytes: &[u8]) -> bool {
    fn string(bytes: &mut &[u8]) -> Option<String> {
        let length = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        let ascii = *bytes.get(4)? == 1;
        *bytes = bytes.get(5..)?;
        if !ascii {
            return None;
        }
        let value = std::str::from_utf8(bytes.get(..length)?).ok()?.to_owned();
        *bytes = bytes.get(length..)?;
        Some(value)
    }
    let Some(scheme) = string(&mut bytes) else {
        return false;
    };
    let Some(host) = string(&mut bytes) else {
        return false;
    };
    (scheme == "tauri" && host == "localhost")
        || ((scheme == "http" || scheme == "https") && host == "tauri.localhost")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(entries: &[(&str, Value)]) -> DesktopPreferences {
        DesktopPreferences {
            available: true,
            values: entries
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }
    fn origin(scheme: &str, host: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in [scheme, host] {
            bytes.extend((value.len() as u32).to_le_bytes());
            bytes.push(1);
            bytes.extend(value.as_bytes());
        }
        bytes
    }

    #[test]
    fn reads_webkit_utf16_and_excludes_development_origins() {
        let root = tempfile::tempdir().unwrap();
        for (name, scheme, label) in [
            ("production", "tauri", "vorkspace 🦀"),
            ("development", "http", "Stale name"),
        ] {
            let path = root.path().join(name).join(name);
            std::fs::create_dir_all(path.join("LocalStorage")).unwrap();
            std::fs::write(path.join("origin"), origin(scheme, "localhost")).unwrap();
            let conn = Connection::open(path.join("LocalStorage/localstorage.sqlite3")).unwrap();
            conn.execute_batch("CREATE TABLE ItemTable(key TEXT,value BLOB)")
                .unwrap();
            let value: Vec<u8> = label.encode_utf16().flat_map(u16::to_le_bytes).collect();
            conn.execute(
                "INSERT INTO ItemTable VALUES(?1,?2)",
                rusqlite::params!["vorkspace.test", value],
            )
            .unwrap();
        }
        let preferences = DesktopPreferences::load_webkit(root.path());
        assert_eq!(preferences.get("vorkspace.test"), Some("vorkspace 🦀"));
        assert!(!desktop_origin(&origin("http", "localhost")));
        assert!(desktop_origin(&origin("https", "tauri.localhost")));
        assert!(!desktop_origin(b"tauri localhost"));
        assert_eq!(decode_webkit_value(&[0]), None);
    }

    #[test]
    fn canonical_preferences_replace_stale_sidecar_projects_and_appearance() {
        let preferences = prefs(&[
            (
                "vorkspace.recentProjects",
                json!([{"path":"/code/agent-terminal"},{"path":"/code/noodle"},{"path":"/code/archived"}]),
            ),
            (
                "vorkspace.projectRailOrder",
                json!(["/code/noodle", "/code/agent-terminal"]),
            ),
            (
                "vorkspace.projectRailPinned",
                json!(["/code/agent-terminal"]),
            ),
            (
                "vorkspace.archivedProjects",
                json!([{"path":"/code/archived"}]),
            ),
            (
                "vorkspace:tab-group:labels",
                json!({"/code/agent-terminal":"vorkspace"}),
            ),
            (
                "vorkspace:tab-group:logos",
                json!({"/code/agent-terminal":"/logos/saved.png"}),
            ),
        ]);
        let mut workspace = json!({"projects":["/tmp/bot","/code/agent-terminal"],"project":"/tmp/bot","active":123,
            "project_appearance":{"/code/agent-terminal":{"name":"stale","background":{"path":"/background.png"}}}});
        preferences.apply_workspace(&mut workspace);
        assert_eq!(
            workspace["projects"],
            json!(["/code/agent-terminal", "/code/noodle"])
        );
        assert_eq!(workspace["project"], "/code/agent-terminal");
        assert!(workspace["active"].is_null());
        assert_eq!(
            workspace["project_appearance"]["/code/agent-terminal"]["name"],
            "vorkspace"
        );
        assert_eq!(
            workspace["project_appearance"]["/code/agent-terminal"]["logo"],
            "/logos/saved.png"
        );
        assert_eq!(
            workspace["project_appearance"]["/code/agent-terminal"]["background"]["path"],
            "/background.png"
        );
    }

    #[test]
    fn native_edits_survive_until_the_corresponding_desktop_setting_changes() {
        let initial = prefs(&[
            (
                "vorkspace.recentProjects",
                json!([{"path":"/code/a"},{"path":"/code/b"}]),
            ),
            ("vorkspace:tab-group:labels", json!({"/code/a":"Desktop"})),
        ]);
        let mut workspace = json!({});
        initial.apply_workspace(&mut workspace);
        workspace["projects"] = json!(["/code/a", "/code/native"]);
        workspace["project_appearance"]["/code/a"]["name"] = json!("Native");
        workspace["project_appearance"]["/code/a"]["mascot"] = json!("cat");
        initial.apply_workspace(&mut workspace);
        assert_eq!(workspace["projects"], json!(["/code/a", "/code/native"]));
        assert_eq!(workspace["project_appearance"]["/code/a"]["name"], "Native");
        assert_eq!(workspace["project_appearance"]["/code/a"]["mascot"], "cat");
        let changed = prefs(&[
            (
                "vorkspace.recentProjects",
                json!([{"path":"/code/a"},{"path":"/code/new"}]),
            ),
            (
                "vorkspace:tab-group:labels",
                json!({"/code/a":"Renamed in desktop"}),
            ),
        ]);
        changed.apply_workspace(&mut workspace);
        assert_eq!(
            workspace["projects"],
            json!(["/code/a", "/code/new", "/code/native"])
        );
        assert_eq!(
            workspace["project_appearance"]["/code/a"]["name"],
            "Renamed in desktop"
        );
        assert_eq!(workspace["project_appearance"]["/code/a"]["mascot"], "cat");
    }

    #[test]
    fn model_control_placement_matches_desktop_and_retains_explicit_native_edits() {
        let beside = DesktopPreferences {
            available: true,
            values: BTreeMap::from([("vorkspace.modelControls".into(), "beside".into())]),
        };
        let mut workspace = json!({"preferences":{"model_controls":"menu"}});
        beside.apply_workspace(&mut workspace);
        assert_eq!(workspace["preferences"]["model_controls"], "beside");
        workspace["preferences"]["model_controls"] = json!("menu");
        beside.apply_workspace(&mut workspace);
        assert_eq!(workspace["preferences"]["model_controls"], "menu");
        let defaults = DesktopPreferences {
            available: true,
            ..Default::default()
        };
        defaults.apply_workspace(&mut workspace);
        assert_eq!(workspace["preferences"]["model_controls"], "menu");
        let legacy = DesktopPreferences {
            available: true,
            values: BTreeMap::from([("vorkspace.composerEffortVisible".into(), "1".into())]),
        };
        legacy.apply_workspace(&mut workspace);
        assert_eq!(workspace["preferences"]["model_controls"], "beside");
    }

    #[test]
    fn desktop_provider_choices_seed_defaults_and_preserve_explicit_native_edits() {
        let source = prefs(&[
            (
                "vorkspace.lastModel",
                json!({"harness":"claude","model":"claude:opus-4.8"}),
            ),
            (
                "vorkspace.projectProviderSettings.v1",
                json!({"/project":{"defaultHarness":"codex"}}),
            ),
        ]);
        let mut workspace = json!({});
        source.apply_workspace(&mut workspace);
        assert_eq!(workspace["preferences"]["default_provider"], "claude");
        assert_eq!(
            workspace["preferences"]["project_providers"]["/project"]["provider"],
            "codex"
        );
        workspace["preferences"]["default_provider"] = json!("codex");
        source.apply_workspace(&mut workspace);
        assert_eq!(workspace["preferences"]["default_provider"], "codex");
        // Removing a desktop override clears its old source-backed value.
        prefs(&[]).apply_workspace(&mut workspace);
        assert!(workspace["preferences"]["default_provider"].is_null());
        assert_eq!(workspace["preferences"]["project_providers"], json!({}));
    }

    #[test]
    fn unavailable_preferences_preserve_the_entire_workspace() {
        let mut workspace = json!({
            "projects": ["/code/native"],
            "project_appearance": {"/code/a":{"name":"Saved"}},
            "desktop_project_preferences": {"appearance":{"/code/a":{"name":"Saved"}}}
        });
        let original = workspace.clone();
        DesktopPreferences::default().apply_workspace(&mut workspace);
        assert_eq!(workspace, original);
    }

    #[test]
    fn isolated_data_directory_cannot_read_desktop_preferences() {
        let directory = tempfile::tempdir().unwrap();
        assert!(DesktopPreferences::load(directory.path()).values.is_empty());
    }
}
