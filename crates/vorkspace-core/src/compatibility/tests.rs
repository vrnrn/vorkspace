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
fn normalizes_keys_without_changing_ids_text_or_unknown_metadata() {
    let old = earlier_prefix();
    assert_eq!(
        key(&format!("{old}:bot-channels")),
        "vorkspace:bot-channels"
    );
    assert_eq!(key("another:bot-channels"), "another:bot-channels");
    let mut block = json!({"role":"user","text":old});
    block[earlier_prefix()] = json!(true);
    assert!(application_block(&block));
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
    assert!(crate::bot::work::command_action(&format!("echo {name} app sessions.start")).is_none());
}
#[test]
fn saved_file_and_provider_home_owners_are_read_and_edited_in_place() {
    let directory = tempfile::tempdir().unwrap();
    let old = earlier_roster_field();
    let current = directory.path().join("bots");
    let earlier = directory.path().join(&old);
    std::fs::create_dir_all(current.join("saved-bot")).unwrap();
    std::fs::create_dir_all(earlier.join("saved-bot/memory")).unwrap();
    std::fs::write(earlier.join("saved-bot/SOUL.md"), "Private identity").unwrap();
    std::fs::write(earlier.join("saved-bot/MEMORY.md"), "- Original memory").unwrap();
    std::fs::write(earlier.join("saved-bot/private-extra.json"), "opaque").unwrap();
    let before = crate::bot::files::load(&current, "saved-bot", None).unwrap();
    assert_eq!(before.soul.as_deref(), Some("Private identity"));
    crate::bot::files::save(
        &current,
        "saved-bot",
        None,
        "MEMORY.md",
        "- Edited memory",
        Some(&before.memory_hash),
    )
    .unwrap();
    assert_eq!(
        crate::bot::files::load(&current, "saved-bot", None)
            .unwrap()
            .memory,
        "- Edited memory"
    );
    assert!(current
        .join("saved-bot")
        .read_dir()
        .unwrap()
        .next()
        .is_none());
    assert_eq!(
        std::fs::read_to_string(earlier.join("saved-bot/private-extra.json")).unwrap(),
        "opaque"
    );
    // Distinct populated current owners keep their own complete identity.
    std::fs::create_dir_all(current.join("new-bot")).unwrap();
    std::fs::create_dir_all(earlier.join("new-bot")).unwrap();
    std::fs::write(current.join("new-bot/SOUL.md"), "Current identity").unwrap();
    std::fs::write(earlier.join("new-bot/SOUL.md"), "Earlier identity").unwrap();
    assert_eq!(
        crate::bot::files::load(&current, "new-bot", None)
            .unwrap()
            .soul
            .as_deref(),
        Some("Current identity")
    );
    let homes = directory.path().join("codex-bots");
    let oldhomes = directory.path().join(format!("codex-{old}"));
    std::fs::create_dir_all(homes.join("default")).unwrap();
    std::fs::create_dir_all(oldhomes.join("default")).unwrap();
    std::fs::write(oldhomes.join("default/state_5.sqlite"), "private state").unwrap();
    assert_eq!(
        crate::bot::codex_home(&current, "default").unwrap(),
        Some(oldhomes.join("default"))
    );
    assert!(homes.join("default").read_dir().unwrap().next().is_none());
    assert_eq!(
        std::fs::read_to_string(oldhomes.join("default/state_5.sqlite")).unwrap(),
        "private state"
    );
}
#[test]
fn earlier_nonempty_collections_outlive_unfinished_empty_placeholders_only() {
    let old = format!(
        "{}:{}-roster",
        earlier_prefix(),
        earlier_title_field().to_lowercase()
    );
    let current = format!("{}:bot-roster", earlier_prefix());
    let source = BTreeMap::from([(old, "[{\"id\":\"kept\"}]".into()), (current, "[]".into())]);
    let aliases = desktop_preferences(source.clone());
    assert_eq!(
        serde_json::from_str::<Value>(&aliases["vorkspace:bot-roster"]).unwrap()[0]["id"],
        "kept"
    );
    let mut finalized = source;
    finalized.insert("vorkspace.botStorageMigration.v1".into(), "1".into());
    assert_eq!(desktop_preferences(finalized)["vorkspace:bot-roster"], "[]");
}
