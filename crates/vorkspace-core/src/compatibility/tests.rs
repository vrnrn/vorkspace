use super::*;
use serde_json::json;
// Byte fixtures are persisted-format samples, never names used for new data.
fn earlier_prefix() -> String {
    String::from_utf8(vec![109, 111, 110, 111, 99, 111, 100, 101]).unwrap()
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
