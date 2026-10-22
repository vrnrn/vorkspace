//! Read compatibility for persisted identities from earlier desktop versions.
//! Fingerprints identify historical names without retaining retired branding in
//! the source tree. Data stays at its existing path; opening never copies a DB,
//! rewrites a transcript, or changes a provider session ID.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

fn fingerprint(name: &str) -> String {
    format!("{:x}", Sha256::digest(name.as_bytes()))
}

fn existing_child(root: &Path, identity: &str) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            (fingerprint(&entry.file_name().to_string_lossy()) == identity).then(|| entry.path())
        })
}

pub(crate) fn desktop_database(base: &Path) -> PathBuf {
    let current = base.join("com.vorkspace.desktop/vorkspace.db");
    if current.exists() {
        return current;
    }
    existing_child(
        base,
        "7bf39d8169a1f5d55d98ef75166e70e97466c5d0762098a86262fb0d419a355b",
    )
    .and_then(|directory| {
        existing_child(
            &directory,
            "9347502a9228f0ec32ca7251ae734c7ddf1a0752334291cd08b93c4efa76c087",
        )
    })
    .filter(|path| path.is_file())
    .unwrap_or(current)
}

/// Normalize only the app-owned key prefix; paths, IDs and user text stay exact.
pub(crate) fn key(key: &str) -> String {
    let end = key.find(['.', ':', '-']).unwrap_or(key.len());
    let prefix = &key[..end];
    let replacement = match fingerprint(prefix).as_str() {
        "01b8c92e5e3a72e0400c920bd7376918189956e379ee280afa4bd4e653df1d9a" => "vorkspace",
        "d7de34b17b4691aa77ab8c17afdd440b004bc87d5d9ab991a8296d032fadb867" => "bot",
        "9b9e74c140f4ffe20089b4929a5609a790039039534159550ce45964efe895f2" => "bots",
        _ => return key.to_owned(),
    };
    format!("{replacement}{}", &key[end..])
}

/// Rename a known field at its owning schema level. Opaque metadata is retained.
fn field(value: &mut Value, identity: &str, current: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let old = object
        .keys()
        .find(|name| fingerprint(name) == identity)
        .cloned();
    if let Some(old) = old {
        if let Some(value) = object.remove(&old) {
            object.entry(current).or_insert(value);
        }
    }
}
fn fields(key: &str, value: &mut Value) {
    if key == "bot-roster" {
        field(
            value,
            "9b9e74c140f4ffe20089b4929a5609a790039039534159550ce45964efe895f2",
            "bots",
        );
        return;
    }
    if key != "workspace" {
        return;
    }
    if let Some(preferences) = value.get_mut("preferences") {
        field(
            preferences,
            "9b9e74c140f4ffe20089b4929a5609a790039039534159550ce45964efe895f2",
            "bots",
        );
        field(
            preferences,
            "edb2c8646842d71551d5076f0dd5b31512986fc05b9c2835f0e7a49bd8511964",
            "rail_bots_pinned",
        );
        field(
            preferences,
            "0fb0ed88a6e8fc8a36af76d9c17f68219b348f29b2d9f83d81e99f96cf631b7d",
            "bot_menu_bar_icon",
        );
    }
    if let Some(tabs) = value.get_mut("title_order").and_then(Value::as_array_mut) {
        for tab in tabs {
            field(
                tab,
                "6f00156cec078cc0c6c207117b68104f4e0e06cf67a3eebc6398b2c7f5248f45",
                "Bot",
            );
        }
    }
    if let Some(scopes) = value.get_mut("scope_focus").and_then(Value::as_object_mut) {
        for tab in scopes.values_mut() {
            field(
                tab,
                "6f00156cec078cc0c6c207117b68104f4e0e06cf67a3eebc6398b2c7f5248f45",
                "Bot",
            );
        }
    }
}

pub(crate) fn application_block(block: &Value) -> bool {
    block.as_object().is_some_and(|object| {
        object
            .iter()
            .any(|(name, value)| value == true && key(name) == "vorkspace")
    })
}

pub(crate) fn settings(
    values: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    let mut result = BTreeMap::new();
    for (old, mut value) in &values {
        let canonical = key(old);
        if old != &canonical && values.contains_key(&canonical) {
            continue;
        }
        let upgraded;
        if matches!(canonical.as_str(), "workspace" | "bot-roster") {
            let mut json: Value =
                serde_json::from_str(value).map_err(|e| format!("Cannot read {canonical}: {e}"))?;
            fields(&canonical, &mut json);
            upgraded = json.to_string();
            value = &upgraded;
        }
        result.insert(canonical, value.clone());
    }
    Ok(result)
}

/// Historical shell receipts can name the earlier executable, but cannot make
/// arbitrary shell commands into accepted app effects.
pub(crate) fn app_executable(name: &str) -> bool {
    matches!(name, "vorkspace" | "vorkspace-gpui") || key(name) == "vorkspace"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    // Byte fixtures are persisted-format samples, never names used for new data.
    fn earlier_prefix() -> String {
        String::from_utf8(vec![109, 111, 110, 111, 99, 111, 100, 101]).unwrap()
    }
    fn earlier_roster_field() -> String {
        String::from_utf8(vec![109, 111, 110, 111, 115]).unwrap()
    }
    fn earlier_title_field() -> String {
        String::from_utf8(vec![77, 111, 110, 111]).unwrap()
    }

    #[test]
    fn finds_exact_prior_database_without_moving_or_copying_it() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join(format!("com.{}.desktop", earlier_prefix()));
        std::fs::create_dir(&folder).unwrap();
        let database = folder.join(format!("{}.db", earlier_prefix()));
        std::fs::write(&database, b"original").unwrap();
        assert_eq!(desktop_database(dir.path()), database);
        assert_eq!(std::fs::read(&database).unwrap(), b"original");
        assert!(!dir.path().join("com.vorkspace.desktop").exists());
        let current = dir.path().join("com.vorkspace.desktop/vorkspace.db");
        std::fs::create_dir_all(current.parent().unwrap()).unwrap();
        std::fs::write(&current, b"current").unwrap();
        assert_eq!(desktop_database(dir.path()), current);
    }
    #[test]
    fn normalizes_keys_without_changing_ids_text_or_unknown_metadata() {
        let old = earlier_prefix();
        assert_eq!(
            key(&format!("{old}:bot-channels")),
            "vorkspace:bot-channels"
        );
        assert_eq!(key("another:bot-channels"), "another:bot-channels");
        let mut roster = json!({"opaque":old});
        roster[earlier_roster_field()] = json!([{"id":"saved-id","name":old}]);
        let mut tab = json!({});
        tab[earlier_title_field()] = json!({"id":"saved-id","project":old});
        let mut preferences = json!({});
        preferences[earlier_roster_field()] = json!(false);
        preferences[format!("rail_{}_pinned", earlier_roster_field())] = json!(true);
        preferences[format!("{}_menu_bar_icon", earlier_title_field().to_lowercase())] =
            json!(false);
        let upgraded = settings(BTreeMap::from([
            ("bot-roster".into(), roster.to_string()),
            (
                "workspace".into(),
                json!({"title_order":[tab], "desktop_workspace_snapshot":roster.clone(), "preferences":preferences})
                    .to_string(),
            ),
            (format!("{old}.preference"), "earlier".into()),
            ("vorkspace.preference".into(), "current".into()),
        ]))
        .unwrap();
        let roster: Value = serde_json::from_str(&upgraded["bot-roster"]).unwrap();
        assert_eq!(roster["bots"][0]["name"], old);
        assert_eq!(roster["opaque"], old);
        let workspace: Value = serde_json::from_str(&upgraded["workspace"]).unwrap();
        assert_eq!(workspace["title_order"][0]["Bot"]["id"], "saved-id");
        assert_eq!(upgraded["vorkspace.preference"], "current");
        assert_eq!(workspace["preferences"]["bots"], false);
        assert_eq!(workspace["preferences"]["rail_bots_pinned"], true);
        assert_eq!(workspace["preferences"]["bot_menu_bar_icon"], false);
        assert!(workspace["desktop_workspace_snapshot"]
            .get(earlier_roster_field())
            .is_some());
        let mut block = json!({"role":"user","text":old});
        block[earlier_prefix()] = json!(true);
        assert!(application_block(&block));
    }
    #[test]
    fn roster_survives_read_modify_save_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vorkspace.db");
        drop(crate::store::Store::open_vorkspace(&path).unwrap());
        let mut roster = json!({});
        roster[earlier_roster_field()] = json!([{"id":"stable-bot","name":"Before"}]);
        let sidecar = path.with_file_name("vorkspace.db.gpui.json");
        std::fs::write(
            &sidecar,
            json!({"bot-roster":roster.to_string()}).to_string(),
        )
        .unwrap();
        let store = crate::store::Store::open_vorkspace(&path).unwrap();
        let mut roster = crate::bot::Roster::load(&store).unwrap();
        assert_eq!(roster.bots.len(), 1);
        roster.get_mut("stable-bot").unwrap().name = "After".into();
        roster.save(&store).unwrap();
        drop(store);
        let reopened = crate::store::Store::open_vorkspace(&path).unwrap();
        let roster = crate::bot::Roster::load(&reopened).unwrap();
        assert_eq!(roster.get("stable-bot").unwrap().name, "After");
        assert_eq!(roster.bots.len(), 1);
    }
    #[test]
    fn malformed_roster_stops_open_without_rewriting_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vorkspace.db");
        drop(crate::store::Store::open_vorkspace(&path).unwrap());
        let sidecar = path.with_file_name("vorkspace.db.gpui.json");
        let raw = json!({"bot-roster":"{"}).to_string();
        std::fs::write(&sidecar, &raw).unwrap();
        assert!(crate::store::Store::open_vorkspace(&path).is_err());
        assert_eq!(std::fs::read_to_string(sidecar).unwrap(), raw);
    }
    #[test]
    fn historical_command_receipts_remain_visible_but_echo_is_not_an_effect() {
        let name = earlier_prefix();
        assert_eq!(
            crate::bot::work::command_action(&format!("{name} app sessions.start")),
            Some("sessions.start".into())
        );
        assert_eq!(
            crate::bot::work::command_action("vorkspace-gpui app sessions.start"),
            Some("sessions.start".into())
        );
        assert!(
            crate::bot::work::command_action(&format!("echo {name} app sessions.start")).is_none()
        );
    }
}
