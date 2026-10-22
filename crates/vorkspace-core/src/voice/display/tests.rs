use super::*;

#[test]
fn only_final_assistant_speech_is_retained_and_bounded() {
    let mut display = Display::default();
    for _ in 0..100 {
        assert!(!display.notification(
            "thread/realtime/transcript/delta",
            &json!({"role":"assistant","delta":"partial"})
        ));
    }
    assert!(!display.notification(
        "thread/realtime/transcript/done",
        &json!({"role":"user","text":"private request"})
    ));
    assert_eq!(display.snapshot()["lines"], json!([]));
    assert!(display.notification(
        "thread/realtime/transcript/done",
        &json!({"role":"assistant","text":"界".repeat(10000)})
    ));
    assert_eq!(display.text.chars().count(), 4000);
    assert!(!display.notification(
        "thread/realtime/transcript/done",
        &json!({"role":"assistant","text":" "})
    ));
    assert!(display.notification(
        "thread/realtime/transcript/done",
        &json!({"role":"assistant","text":"Latest reply"})
    ));
    assert_eq!(
        display.snapshot()["lines"],
        json!([{"text":"Latest reply"}])
    );
}

#[test]
fn stale_completions_do_not_end_new_playback() {
    let mut d = Display::default();
    d.realtime(&json!({"type":"turn.created","turn":{"id":"old","role":"assistant","start_ms":0,"end_ms":200}}));
    d.notification("turn/completed", &json!({}));
    assert_eq!(d.snapshot()["activity"], "replying");
    d.mood = "happy".into();
    d.realtime(&json!({"type":"turn.created","turn":{"id":"user","role":"user","start_ms":200,"end_ms":400}}));
    assert_eq!(d.snapshot()["activity"], "listening");
    assert!(d.mood.is_empty());
    d.realtime(
        &json!({"type":"turn.done","turn":{"id":"user","role":"user","start_ms":200,"end_ms":800}}),
    );
    assert_eq!(d.snapshot()["activity"], "thinking");
    d.realtime(&json!({"type":"output_transcript.added","start_ms":1000}));
    d.realtime(&json!({"type":"turn.done","turn":{"id":"old","role":"assistant","start_ms":0,"end_ms":200}}));
    d.notification("turn/completed", &json!({}));
    assert_eq!(d.snapshot()["activity"], "replying");
    d.realtime(&json!({"type":"turn.done","turn":{"id":"new","role":"assistant","start_ms":1000,"end_ms":2000}}));
    assert_eq!(d.snapshot()["activity"], "listening");
}

#[test]
fn context_is_chronological_text_only_and_explicitly_past() {
    let page = json!({"data":[{"items":[{"type":"agentMessage","text":"later"},{"type":"commandExecution","text":"tool output"}]},{"items":[{"type":"userMessage","content":[{"type":"text","text":"earlier"},{"type":"image","text":"image payload"}]}]}]});
    let context = recent_context(&page);
    let text = context[0]["text"].as_str().unwrap();
    assert!(text.contains("not requests to act again"));
    assert!(text.contains("user: earlier\n\nassistant: later"));
    assert!(!text.contains("tool output"));
    assert!(!text.contains("image payload"));
    assert_eq!(context[0]["role"], "assistant");
}

#[test]
fn context_bounds_unicode_without_mutating_history() {
    let page = json!({"data":[{"items":[{"type":"agentMessage","text":format!("{}recent end","🦀".repeat(20000))}]}]});
    let original = page.clone();
    let context = recent_context(&page);
    let text = context[0]["text"].as_str().unwrap();
    assert!(text.chars().count() < 3200);
    assert!(text.ends_with("recent end"));
    assert_eq!(page, original);
    assert_eq!(recent_context(&Value::Null), json!([]));
    assert_eq!(recent_context(&json!({"data":[]})), json!([]));
}
