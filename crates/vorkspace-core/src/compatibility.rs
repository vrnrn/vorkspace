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

/// Normalize app-owned persisted identities only. Paths, IDs and user text stay exact.
pub(crate) fn key(key: &str) -> String {
    let end = key.find(['.', ':', '-']).unwrap_or(key.len());
    let prefix = &key[..end];
    let replacement = match fingerprint(prefix).as_str() {
        "01b8c92e5e3a72e0400c920bd7376918189956e379ee280afa4bd4e653df1d9a" => "vorkspace",
        "d7de34b17b4691aa77ab8c17afdd440b004bc87d5d9ab991a8296d032fadb867" => "bot",
        "9b9e74c140f4ffe20089b4929a5609a790039039534159550ce45964efe895f2" => "bots",
        _ => prefix,
    };
    let suffix = &key[end..];
    if replacement == "vorkspace" {
        return format!("{replacement}{}", desktop_feature_key(suffix));
    }
    format!("{replacement}{suffix}")
}
fn desktop_feature_key(suffix: &str) -> String {
    let current = match fingerprint(suffix).as_str() {
        "4a1d8d72a19645e07228635fbe24e93b7c59b1d45994b2381af4b480a2cef5d1" => Some(":bot-roster"),
        "9409fc5805b3f7f6a068dfccfe901785cd57ca40510e21f9cc66150f895de923" => Some(":bot-handed"),
        "6d6da5426e131be03170f2bc63abd977865cf84f77b3168973c657bcfe1279bc" => Some(":bots"),
        "1c1c523b3ae0fd0136ab809e5bc8c7229baae14fe3c43d600f981743b15e3611" => Some(":bot-profiles"),
        "a5a571611b45a9ef4ebc454edb9b45ab983067b56748d42325dd9e6398c502bf" => Some(":open-bot"),
        "54192bdcda5a0a7d4cff841181dd4fa50432a7a70d8aff5bfff793f07f167245" => {
            Some(":bot-declined-habits")
        }
        "439dc81975b7a860125d40056a8437fdbf79427631d19b70f3b07558924d808d" => Some(":bot-channels"),
        "b14718f5973225b79168b6aa9d2c547661341d0f1f3fc69382a73c7f7b9a69b4" => {
            Some(":open-bot-channel")
        }
        "4c8b8b6f1376d1c15ee7173384ac2e59905c9e947aa311c1e8e4dacc33dde30a" => {
            Some(":bot-rotations")
        }
        "2e2939a4208ff3b97e6596dfdbbf9f3c1ffa00e39919da8104a5c977e845447d" => {
            Some(":bot-intro-dismissed")
        }
        "a1420c2c8c7cd1b16994778243c5089c706344b9522959f7fc7ffd68f9f6c3f1" => Some(".inboxBots.v1"),
        "530bf8cfe8044cf3ea2bf4fd2a1432ad52911c8355207a400ff534685521b42f" => {
            Some(".railBotsPinned")
        }
        "3f600b35a79f3252c0d9c1c05c7cc97cab0f6f6368e45350a917cf8e6d99229e" => Some(".botsEnabled"),
        "2b1c91a8f2105421e0aaef80649c463917f32dda9c92f5befaba75885f469a75" => {
            Some(".botMenuBarIcon")
        }
        _ => None,
    };
    if let Some(current) = current {
        return current.to_owned();
    }
    if let Some((feature, id)) = suffix.rsplit_once(':') {
        if fingerprint(feature)
            == "c5472cf2ccd8620cb9c2e2560cf0f7f35b57ac55cd23cdfe751bdc4dea198a37"
        {
            return format!(":bot-channel-messages:{id}");
        }
    }
    suffix.to_owned()
}

/// Read aliases stay derived; WebKit rows, opaque metadata and arbitrary text
/// are never rewritten. Current feature identities outrank earlier aliases.
pub(crate) fn desktop_preferences(values: BTreeMap<String, String>) -> BTreeMap<String, String> {
    let finalized = values
        .iter()
        .any(|(name, value)| key(name) == "vorkspace.botStorageMigration.v1" && value == "1");
    let mut earlier_collections = BTreeMap::new();
    let mut chosen = BTreeMap::<String, (u8, String)>::new();
    for (old, raw) in values {
        let canonical = key(&old);
        if !canonical.starts_with("vorkspace.") && !canonical.starts_with("vorkspace:") {
            continue;
        }
        let end = old.find(['.', ':', '-']).unwrap_or(old.len());
        let feature_current = old[end..] == desktop_feature_key(&old[end..]);
        let priority = (feature_current as u8) * 2 + u8::from(old == canonical);
        if !feature_current && !finalized {
            if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                if value.as_array().is_some_and(|v| !v.is_empty())
                    || value.as_object().is_some_and(|v| !v.is_empty())
                {
                    earlier_collections.insert(canonical.clone(), raw.clone());
                }
            }
        }
        if chosen
            .get(&canonical)
            .is_none_or(|(rank, _)| priority > *rank)
        {
            chosen.insert(canonical, (priority, raw));
        }
    }
    for (key, (_, raw)) in &mut chosen {
        let collection = matches!(
            key.as_str(),
            "vorkspace:bot-roster"
                | "vorkspace:bot-channels"
                | "vorkspace:bot-declined-habits"
                | "vorkspace:bot-handed"
                | "vorkspace:bots"
                | "vorkspace:bot-profiles"
                | "vorkspace:bot-rotations"
                | "vorkspace.inboxBots.v1"
        ) || key.starts_with("vorkspace:bot-channel-messages:");
        if collection
            && serde_json::from_str::<Value>(raw).ok().is_some_and(|v| {
                v.as_array().is_some_and(Vec::is_empty)
                    || v.as_object().is_some_and(serde_json::Map::is_empty)
            })
        {
            if let Some(earlier) = earlier_collections.remove(key) {
                *raw = earlier;
            }
        }
    }
    chosen
        .into_iter()
        .map(|(key, (_, raw))| {
            let raw = desktop_value(&key, &raw).unwrap_or(raw);
            (key, raw)
        })
        .collect()
}
fn desktop_value(key: &str, raw: &str) -> Option<String> {
    if key == "vorkspace.settingsSection"
        && fingerprint(raw) == "9b9e74c140f4ffe20089b4929a5609a790039039534159550ce45964efe895f2"
    {
        return Some("bots".into());
    }
    if !matches!(
        key,
        "vorkspace:bot-channels"
            | "vorkspace:project-chat-backgrounds"
            | "vorkspace.keybindingOverrides"
    ) && !key.starts_with("vorkspace:bot-channel-messages:")
    {
        return None;
    }
    let mut value: Value = serde_json::from_str(raw).ok()?;
    if key == "vorkspace:bot-channels" {
        for row in value.as_array_mut().into_iter().flatten() {
            field(
                row,
                "25c8a943a204cf96ade0cbcdb59c5432a98057fefe2092d48ace24b8c2c27acd",
                "botIds",
            );
        }
    } else if key.starts_with("vorkspace:bot-channel-messages:") {
        for row in value.as_array_mut().into_iter().flatten() {
            for name in ["from", "author"] {
                if let Some(author) = row.get_mut(name) {
                    if author["kind"].as_str().is_some_and(|s|fingerprint(s)=="d7de34b17b4691aa77ab8c17afdd440b004bc87d5d9ab991a8296d032fadb867") { author["kind"] = "bot".into(); }
                    field(
                        author,
                        "cb7384ab0f67c31369b50c300c861c0f859e584a838e951afad6f9738414f3d6",
                        "botId",
                    );
                }
            }
            for reaction in row
                .get_mut("reactions")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                field(
                    reaction,
                    "cb7384ab0f67c31369b50c300c861c0f859e584a838e951afad6f9738414f3d6",
                    "botId",
                );
            }
        }
    } else if let Some(object) = value.as_object_mut() {
        for old in object.keys().cloned().collect::<Vec<_>>() {
            let next = if key == "vorkspace.keybindingOverrides"
                && fingerprint(&old)
                    == "cb20b3759dccefa7d8ea202f2dc10ea4275b26a9e80e24aee47019b7c993ea8a"
            {
                Some("App: Toggle Bot".to_owned())
            } else if key == "vorkspace:project-chat-backgrounds" {
                old.split_once(':')
                    .filter(|(prefix, _)| {
                        fingerprint(prefix)
                            == "d7de34b17b4691aa77ab8c17afdd440b004bc87d5d9ab991a8296d032fadb867"
                    })
                    .map(|(_, id)| format!("bot:{id}"))
            } else {
                None
            };
            if let Some(next) = next {
                if let Some(value) = object.remove(&old) {
                    object.entry(next).or_insert(value);
                }
            }
        }
    }
    Some(value.to_string())
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

/// Only known app-owned metadata is interpreted; nested tool payloads and text
/// remain opaque, including when ordinary edits save the block again.
pub(crate) fn block(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        for old in object
            .keys()
            .filter(|name| {
                !name.starts_with("bot")
                    && [
                        "Habit",
                        "Card",
                        "Reaction",
                        "SessionCompletion",
                        "PeerMessage",
                        "SpawnedSessions",
                    ]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
            })
            .cloned()
            .collect::<Vec<_>>()
        {
            let current = match fingerprint(&old).as_str() {
                "2c658edc2b1c279e0e09d2a05509361ca3d256bd1f43c64a2715ca52bb8e3206" => {
                    Some("botHabit")
                }
                "4dd1106359c54269d46ad2f71efe553e9a97ca285d4a6daea78a1c230e959c9b" => {
                    Some("botCard")
                }
                "2d7aad824eab31db08aad2976d73ed6768a7833fa79b93aa50076fdfd71f28dd" => {
                    Some("botReaction")
                }
                "5f238f25e2f3d93b29cceaae8d8db76cdeef5ae8147d7481e7d587767b36fc78" => {
                    Some("botSessionCompletion")
                }
                "d131ac3c80a76da275d7e6b0345ee7bf5314b03872b23a2ceeb01460c028fb44" => {
                    Some("botPeerMessage")
                }
                "409c710995f90531f0d2dc1b598d72beecef67ef4825ed53d0c0cc5c74ab934d" => {
                    Some("botSpawnedSessions")
                }
                _ => None,
            };
            if let Some(current) = current {
                if let Some(value) = object.remove(&old) {
                    object.entry(current).or_insert(value);
                }
            }
        }
    }
    if let Some(peer) = value.get_mut("botPeerMessage") {
        field(
            peer,
            "a367212661cd53967e23173f783720ddc743bf4a7f011cbc52dbe54c67e9a875",
            "fromBotId",
        );
        field(
            peer,
            "63befccfd967bc398ba39b3b5b0b06fd368688fcb933f61a1e97c9f9dad67e13",
            "toBotId",
        );
    }
    for thread in value
        .get_mut("btwThreads")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        for message in thread
            .get_mut("messages")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            for child in message
                .get_mut("blocks")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                block(child);
            }
        }
    }
}

/// Select an existing complete owner in place. An empty counterpart directory
/// does not replace a saved owner; two populated owners are never combined.
pub(crate) fn has_owner_records(path: &Path, depth: u8) -> bool {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
        return false;
    }
    std::fs::read_dir(path).ok().is_some_and(|entries| {
        entries.flatten().any(|entry| {
            entry.file_type().map_or(true, |kind| {
                !kind.is_dir() || depth == 0 || has_owner_records(&entry.path(), depth - 1)
            })
        })
    })
}

pub(crate) fn bot_families(root: &Path, provider_home: bool) -> Vec<PathBuf> {
    let mut paths = vec![root.to_owned()];
    let identity = if provider_home {
        "3c5593b09cae5aab4027ccb4da4eaa3d2ab63276e3566466341b6769acc7be1b"
    } else {
        "9b9e74c140f4ffe20089b4929a5609a790039039534159550ce45964efe895f2"
    };
    if let Some(earlier) = root
        .parent()
        .and_then(|base| existing_child(base, identity))
        .filter(|path| {
            std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        })
    {
        paths.push(earlier);
    }
    paths
}
pub(crate) fn bot_owner(root: &Path, owner: &str, provider_home: bool) -> PathBuf {
    let current = root.join(owner);
    if has_owner_records(&current, 4) {
        return current;
    }
    for earlier in bot_families(root, provider_home)
        .into_iter()
        .skip(1)
        .map(|p| p.join(owner))
    {
        if has_owner_records(&earlier, 4) {
            return earlier;
        }
    }
    current
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
mod tests;
