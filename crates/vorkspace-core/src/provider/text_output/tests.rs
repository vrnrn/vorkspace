use super::*;
use serde_json::json;

fn codex_message(output: &mut Output, phase: Value, text: &str) {
    output
        .receive(&json!({"method":"item/completed", "params":{"item":{
            "type":"agentMessage", "id":"message", "phase":phase, "text":text
        }}}))
        .unwrap();
}

fn complete_codex(output: &mut Output) {
    output
        .receive(&json!({"method":"turn/completed", "params":{"turn":{"status":"completed"}}}))
        .unwrap();
}

#[test]
fn codex_selects_completed_final_and_keeps_its_exact_content() {
    let mut output = Output::new(Kind::Codex);
    codex_message(&mut output, json!("commentary"), &"progress".repeat(20_000));
    output
        .receive(
            &json!({"method":"item/reasoning/textDelta", "params":{"delta":"private reasoning"}}),
        )
        .unwrap();
    codex_message(&mut output, json!("partial_answer"), "Preliminary notes");
    output.receive(&json!({"method":"item/agentMessage/delta", "params":{"itemId":"final", "delta":"streamed text is not the final snapshot"}})).unwrap();
    let notes = "## Discussion\n- Keep the team's phrase: ‘thinking about launch’.";
    codex_message(&mut output, json!("final_answer"), notes);
    complete_codex(&mut output);
    assert_eq!(output.finish().unwrap(), notes);
}

#[test]
fn codex_legacy_models_use_only_the_last_completed_unknown_phase_message() {
    let mut output = Output::new(Kind::Codex);
    codex_message(&mut output, Value::Null, "I will prepare the notes.");
    codex_message(&mut output, Value::Null, "## Summary\nFinal notes.");
    complete_codex(&mut output);
    assert_eq!(output.finish().unwrap(), "## Summary\nFinal notes.");
}

#[test]
fn explicit_commentary_or_partial_answer_cannot_be_a_legacy_final() {
    for phase in ["commentary", "partial_answer", "unknown-future-phase"] {
        let mut output = Output::new(Kind::Codex);
        codex_message(&mut output, Value::Null, "Earlier unclassified text");
        codex_message(&mut output, json!(phase), "Progress, not notes");
        complete_codex(&mut output);
        assert!(output.finish().unwrap_err().contains("no completed final"));
    }
}

#[test]
fn an_empty_or_unfinished_explicit_final_never_falls_back_to_other_text() {
    for completed in [false, true] {
        let mut output = Output::new(Kind::Codex);
        codex_message(&mut output, Value::Null, "Earlier unknown-phase text");
        codex_message(&mut output, json!("final_answer"), "Earlier final");
        output.receive(&json!({"method":if completed {"item/completed"} else {"item/started"}, "params":{"item":{
            "type":"agentMessage", "id":"replacement", "phase":"final_answer", "text":"  "
        }}})).unwrap();
        codex_message(&mut output, Value::Null, "Later unknown-phase text");
        complete_codex(&mut output);
        assert!(output.finish().is_err());
    }
}

#[test]
fn uncompleted_deltas_cannot_become_notes_even_after_successful_turn_completion() {
    let mut output = Output::new(Kind::Codex);
    output.receive(&json!({"method":"item/agentMessage/delta", "params":{"itemId":"answer", "delta":"unfinished"}})).unwrap();
    complete_codex(&mut output);
    assert!(output.finish().unwrap_err().contains("no completed final"));

    let mut output = Output::new(Kind::Codex);
    codex_message(&mut output, Value::Null, "Earlier completed message");
    output
        .receive(&json!({"method":"item/started", "params":{"item":{
            "type":"agentMessage", "id":"replacement", "phase":null
        }}}))
        .unwrap();
    complete_codex(&mut output);
    assert!(output.finish().unwrap_err().contains("no completed final"));
}

#[test]
fn codex_terminal_snapshot_supplies_final_only_when_the_turn_succeeds() {
    for status in ["completed", "failed", "interrupted", "inProgress"] {
        let mut output = Output::new(Kind::Codex);
        codex_message(&mut output, json!("final_answer"), "Already received final");
        output
            .receive(&json!({"method":"turn/completed", "params":{"turn":{
                "status":status, "items":[
                    {"type":"reasoning", "text":"reasoning"},
                    {"type":"agentMessage", "phase":"commentary", "text":"progress"},
                    {"type":"agentMessage", "phase":"final_answer", "text":"Final snapshot"}
                ]
            }}}))
            .unwrap();
        if status == "completed" {
            assert_eq!(output.finish().unwrap(), "Final snapshot");
        } else {
            assert!(output.finish().is_err(), "{status}");
        }
    }
}

#[test]
fn claude_uses_successful_result_instead_of_partial_assistant_or_thinking_blocks() {
    for (result, failed) in [
        ("## Summary\nFinal notes", false),
        ("", false),
        ("Failure text", true),
    ] {
        let mut output = Output::new(Kind::Claude);
        output
            .receive(&json!({"type":"assistant", "message":{"content":[
                {"type":"thinking", "thinking":"private reasoning"},
                {"type":"text", "text":"I will write the notes."}
            ]}}))
            .unwrap();
        output
            .receive(&json!({"type":"result", "is_error":failed, "result":result}))
            .unwrap();
        if !failed && !result.is_empty() {
            assert_eq!(output.finish().unwrap(), result);
        } else {
            assert!(output.finish().is_err());
        }
    }
}

#[test]
fn pi_and_omp_select_only_completed_text_blocks_from_the_last_assistant_message() {
    for kind in [Kind::Pi, Kind::Omp] {
        let mut output = Output::new(kind);
        output.receive(&json!({"type":"message_end", "message":{"role":"assistant", "stopReason":"stop", "content":[{"type":"text", "text":"Progress"}]}})).unwrap();
        output.receive(&json!({"type":"message_end", "message":{"role":"assistant", "stopReason":"stop", "content":[
            {"type":"thinking", "thinking":"private reasoning"},
            {"type":"text", "text":"## Summary\n"},
            {"type":"text", "text":"Final notes"}
        ]}})).unwrap();
        assert_eq!(output.finish().unwrap(), "## Summary\nFinal notes");
    }
}

#[test]
fn pi_retries_unfinished_replacements_and_failed_messages_discard_earlier_text() {
    for next in [
        json!({"type":"auto_retry_start"}),
        json!({"type":"agent_end", "willRetry":true}),
        json!({"type":"message_start", "message":{"role":"assistant"}}),
        json!({"type":"message_end", "message":{"role":"assistant", "stopReason":"error"}}),
        json!({"type":"message_end", "message":{"role":"assistant", "stopReason":"aborted"}}),
        json!({"type":"message_end", "message":{"role":"assistant", "stopReason":"length"}}),
    ] {
        let mut output = Output::new(Kind::Pi);
        output.receive(&json!({"type":"message_end", "message":{"role":"assistant", "stopReason":"stop", "content":[{"type":"text", "text":"Earlier message"}]}})).unwrap();
        output.receive(&next).unwrap();
        assert!(output.finish().is_err(), "{next}");
    }
}

#[test]
fn final_response_budget_counts_all_text_blocks_but_no_thinking_blocks() {
    let mut output = Output::new(Kind::Pi);
    output.receive(&json!({"type":"message_end", "message":{"role":"assistant", "stopReason":"stop", "content":[
        {"type":"thinking", "thinking":"x".repeat(LIMIT * 2)},
        {"type":"text", "text":"Final notes"}
    ]}})).unwrap();
    assert_eq!(output.finish().unwrap(), "Final notes");

    let mut output = Output::new(Kind::Pi);
    let error = output.receive(&json!({"type":"message_end", "message":{"role":"assistant", "stopReason":"stop", "content":[
        {"type":"text", "text":"x".repeat(LIMIT)},
        {"type":"text", "text":"overflow"}
    ]}})).unwrap_err();
    assert!(error.contains("exceeded its limit"));
}
