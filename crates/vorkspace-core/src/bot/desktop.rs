//! Desktop identities live in the workspace webview, while their conversations
//! and provider configuration remain in the shared desktop database.
use super::*;
use crate::desktop_preferences::DesktopPreferences;
use serde_json::Value;

const IDENTITY_FIELDS: &[&str] = &[
    "sessionId",
    "unavailableSession",
    "name",
    "mascot",
    "color",
    "projects",
    "showStartedSessionsInSidebar",
    "useSidebarFolders",
    "background",
];

impl Roster {
    pub(super) fn reconcile_desktop(
        &mut self,
        store: &Store,
        preferences: &DesktopPreferences,
    ) -> Result<(), String> {
        let Some(raw) = preferences.get("vorkspace:bot-roster") else {
            return Ok(());
        };
        let roster: Value = serde_json::from_str(raw)
            .map_err(|e| format!("Cannot read desktop Bot roster: {e}"))?;
        let entries = roster
            .as_array()
            .ok_or("Desktop Bot roster is not an array")?;
        let backgrounds = preferences.json("vorkspace:project-chat-backgrounds");
        let source = entries
            .iter()
            .filter_map(|value| desktop_bot(store, value, backgrounds.as_ref()))
            .collect();
        self.merge_desktop(source)
    }

    fn merge_desktop(&mut self, source: Vec<Bot>) -> Result<(), String> {
        let previous = self.desktop_source.take().unwrap_or_default();
        let previous_order: Vec<_> = previous
            .iter()
            .filter(|m| self.get(&m.id).is_some() && source.iter().any(|s| s.id == m.id))
            .map(|m| &m.id)
            .collect();
        let local_order: Vec<_> = self
            .bots
            .iter()
            .filter(|m| {
                previous.iter().any(|p| p.id == m.id) && source.iter().any(|s| s.id == m.id)
            })
            .map(|m| &m.id)
            .collect();
        let locally_reordered = previous_order != local_order;
        let mut merged = Vec::new();
        let mut seen = HashSet::new();
        for incoming in &source {
            if !seen.insert(incoming.id.clone()) {
                continue;
            }
            let old = previous.iter().find(|m| m.id == incoming.id);
            let saved = self.get(&incoming.id);
            // An identity present in the baseline and absent locally was
            // deliberately deleted in native UI. Do not recreate it on open.
            if old.is_some() && saved.is_none() {
                continue;
            }
            let mut value =
                serde_json::to_value(saved.unwrap_or(incoming)).map_err(|e| e.to_string())?;
            let latest = serde_json::to_value(incoming).map_err(|e| e.to_string())?;
            let baseline = old
                .map(serde_json::to_value)
                .transpose()
                .map_err(|e| e.to_string())?;
            for &field in IDENTITY_FIELDS {
                // Preserve native changes only when a baseline proves that
                // the native value was changed. Old sidecars never hide the
                // authoritative roster on the first compatible launch.
                if baseline
                    .as_ref()
                    .is_none_or(|old| value.get(field) == old.get(field))
                {
                    if let Some(next) = latest.get(field) {
                        value[field] = next.clone();
                    } else if let Some(object) = value.as_object_mut() {
                        object.remove(field);
                    }
                }
            }
            merged.push(serde_json::from_value(value).map_err(|e| e.to_string())?);
        }
        // Native-created identities have no desktop baseline. Desktop removals
        // disappear; native identities retain their original UUID and state.
        merged.extend(
            self.bots
                .iter()
                .filter(|bot| {
                    !source.iter().any(|m| m.id == bot.id)
                        && !previous.iter().any(|m| m.id == bot.id)
                })
                .cloned(),
        );
        if locally_reordered {
            merged.sort_by_key(|bot| {
                self.bots
                    .iter()
                    .position(|m| m.id == bot.id)
                    .unwrap_or(self.bots.len())
            });
        }
        self.bots = merged;
        self.desktop_source = Some(source);
        Ok(())
    }
}

fn desktop_bot(store: &Store, value: &Value, backgrounds: Option<&Value>) -> Option<Bot> {
    let id = value.get("id")?.as_str()?;
    if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return None;
    }
    let session = value
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let session_id = session.and_then(|id| store.resolve_session_id(id).ok());
    let background = backgrounds
        .and_then(|all| all.get(format!("bot:{id}")))
        .and_then(|entry| entry.get("path"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(|path| Background {
            path: path.into(),
            effect: crate::project_appearance::Effect::GradientBlur,
            ..Default::default()
        });
    let mut bot = Bot {
        id: id.into(),
        session_id,
        unavailable_session: session.filter(|_| session_id.is_none()).map(str::to_owned),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        mascot: value
            .get("mascot")
            .and_then(Value::as_str)
            .unwrap_or(MASCOTS[0])
            .into(),
        color: value
            .get("color")
            .and_then(Value::as_str)
            .and_then(css_color)
            .unwrap_or(COLORS_BOT[0]),
        projects: value
            .get("projects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        show_started_sessions_in_sidebar: value
            .get("showStartedSessionsInSidebar")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        use_sidebar_folders: value
            .get("useSidebarFolders")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        background,
        ..Default::default()
    };
    bot.normalize();
    Some(bot)
}

/// The desktop palette/custom picker persists CSS HSL or six-digit hex.
fn css_color(raw: &str) -> Option<u32> {
    let raw = raw.trim();
    if let Some(hex) = raw.strip_prefix('#').filter(|s| s.len() == 6) {
        return u32::from_str_radix(hex, 16).ok();
    }
    let hsl = raw.strip_prefix("hsl(")?.strip_suffix(')')?;
    let values: Vec<_> = hsl
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect();
    if values.len() != 3 {
        return None;
    }
    let hue = values[0].parse::<f64>().ok()?;
    let saturation = values[1].strip_suffix('%')?.parse::<f64>().ok()? / 100.;
    let lightness = values[2].strip_suffix('%')?.parse::<f64>().ok()? / 100.;
    if !hue.is_finite() || !saturation.is_finite() || !lightness.is_finite() {
        return None;
    }
    let hue = hue.rem_euclid(360.) / 60.;
    let lightness = lightness.clamp(0., 1.);
    let chroma = (1. - (2. * lightness - 1.).abs()) * saturation.clamp(0., 1.);
    let x = chroma * (1. - (hue % 2. - 1.).abs());
    let (r, g, b) = match hue as u8 {
        0 => (chroma, x, 0.),
        1 => (x, chroma, 0.),
        2 => (0., chroma, x),
        3 => (0., x, chroma),
        4 => (x, 0., chroma),
        _ => (chroma, 0., x),
    };
    let offset = lightness - chroma / 2.;
    let byte = |v: f64| ((v + offset) * 255.).round() as u32;
    Some((byte(r) << 16) | (byte(g) << 8) | byte(b))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn desktop_identity_retains_uuid_session_configuration_and_appearance() {
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let original = store.persisted_session_id(session.id).unwrap();
        let bot = desktop_bot(
            &store,
            &json!({
                "id":"bot-original", "sessionId":original, "name":"Big Boss", "mascot":"ghost",
                "color":"hsl(330 70% 62%)", "projects":["/project"], "useSidebarFolders":true
            }),
            None,
        )
        .unwrap();
        assert_eq!(bot.id, "bot-original");
        assert_eq!(bot.session_id, Some(session.id));
        assert_eq!(bot.name, "Big Boss");
        assert_eq!(bot.mascot, "ghost");
        assert_eq!(bot.color, 0xe25a9e);
        assert!(bot.use_sidebar_folders);
        assert_eq!(
            store.persisted_session_id(bot.session_id.unwrap()).unwrap(),
            original
        );
        assert_eq!(store.sessions().unwrap().len(), 1);
    }

    #[test]
    fn missing_desktop_session_never_becomes_an_unclaimed_new_bot() {
        let store = Store::open_vorkspace(Path::new(":memory:")).unwrap();
        let bot = desktop_bot(
            &store,
            &json!({"id":"saved-bot", "sessionId":"missing-uuid"}),
            None,
        )
        .unwrap();
        assert_eq!(bot.unavailable_session.as_deref(), Some("missing-uuid"));
        assert!(bot.session_id.is_none());
        assert!(store.sessions().unwrap().is_empty());
    }

    #[test]
    fn source_updates_merge_with_native_edits_runtime_and_deletions_across_restarts() {
        let mut roster = Roster::default();
        let mut first = Bot {
            id: "existing".into(),
            name: "Original".into(),
            ..Default::default()
        };
        let removed = Bot {
            id: "removed".into(),
            ..Default::default()
        };
        roster
            .merge_desktop(vec![first.clone(), removed.clone()])
            .unwrap();
        roster.get_mut("existing").unwrap().name = "Native rename".into();
        roster.get_mut("existing").unwrap().seen_rows = 42;
        roster.bots.retain(|m| m.id != "removed");
        let raw = serde_json::to_string(&roster).unwrap();
        let mut roster: Roster = serde_json::from_str(&raw).unwrap();
        first.name = "Desktop rename".into();
        first.mascot = "rocket".into();
        first.session_id = Some(51);
        roster.merge_desktop(vec![first, removed]).unwrap();
        let bot = roster.get("existing").unwrap();
        assert_eq!(bot.name, "Native rename");
        assert_eq!(bot.mascot, "rocket");
        assert_eq!(bot.session_id, Some(51));
        assert_eq!(bot.seen_rows, 42);
        assert!(roster.get("removed").is_none());
        roster.merge_desktop(Vec::new()).unwrap();
        assert!(roster.bots.is_empty());
    }

    #[test]
    fn codex_home_is_account_scoped_and_never_created_by_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("bots");
        assert!(codex_home(&root, "default").unwrap().is_none());
        let expected = temp.path().join("codex-bots/default");
        std::fs::create_dir_all(&expected).unwrap();
        assert_eq!(codex_home(&root, "default").unwrap(), Some(expected));
        assert!(codex_home(&root, "another-account").unwrap().is_none());
        assert!(codex_home(&root, "../escape").is_err());
    }

    #[test]
    fn native_rail_order_survives_source_refresh_and_new_desktop_identity() {
        let bot = |id: &str| Bot {
            id: id.into(),
            ..Default::default()
        };
        let mut roster = Roster::default();
        roster.merge_desktop(vec![bot("one"), bot("two")]).unwrap();
        roster.reorder(&["two".into(), "one".into()]);
        roster
            .merge_desktop(vec![bot("one"), bot("two"), bot("three")])
            .unwrap();
        assert_eq!(
            roster
                .bots
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "one", "three"]
        );
    }
}
