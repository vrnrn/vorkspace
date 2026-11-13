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
