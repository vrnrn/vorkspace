use super::*;
#[test]
fn unsupported_history_never_resolves_or_starts_a_provider() {
    for id in [
        "cursor", "grok", "opencode", "fx", "hermes", "devin", "unknown",
    ] {
        let kind = Kind::from_id(id);
        assert!(resolve(kind).is_none());
        assert!(resolve_auto(kind).is_none());
        let error = run(
            Request {
                kind,
                binary: "must-not-run".into(),
                cwd: "/missing-historical-project".into(),
                prompt: "Continue".into(),
                provider_id: Some("preserved-thread".into()),
                model: Some("preserved-model".into()),
                settings: Default::default(),
            },
            Arc::new(AtomicBool::new(false)),
            |_| panic!("unsupported history must fail before publishing provider events"),
        )
        .unwrap_err();
        assert!(error.contains("no longer supported"), "{error}");
    }
}
fn protocol(kind: Kind) -> Protocol {
    Protocol::new(Request {
        kind,
        binary: "unused".into(),
        cwd: "/tmp".into(),
        prompt: "Hello".into(),
        provider_id: None,
        model: None,
        settings: Default::default(),
    })
}
#[test]
fn context_reports_use_the_latest_request_and_include_cached_tokens() {
    let mut codex = protocol(Kind::Codex);
    let events = codex.frame(json!({"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"last":{"totalTokens":4200},"total":{"totalTokens":99000},"modelContextWindow":272000}}}),&mut Vec::new()).unwrap();
    assert_eq!(
        events,
        vec![Event::Context {
            used: Some(4200),
            window: Some(272000)
        }]
    );
    let mut claude = protocol(Kind::Claude);
    let events = claude.frame(json!({"type":"assistant","message":{"content":[],"usage":{"input_tokens":100,"cache_read_input_tokens":9000,"cache_creation_input_tokens":500,"output_tokens":25}}}),&mut Vec::new()).unwrap();
    assert_eq!(
        events,
        vec![Event::Context {
            used: Some(9625),
            window: None
        }]
    );
    assert!(claude.frame(json!({"type":"assistant","parent_tool_use_id":"subagent","message":{"content":[],"usage":{"input_tokens":999999}}}),&mut Vec::new()).unwrap().is_empty());
    let events = claude.frame(json!({"type":"result","usage":{"input_tokens":90000,"iterations":[{"input_tokens":1000},{"input_tokens":2000,"output_tokens":50}]},"modelUsage":{"model":{"contextWindow":200000}}}),&mut Vec::new()).unwrap();
    assert!(events.contains(&Event::Context {
        used: Some(2050),
        window: Some(200000)
    }));
}
#[test]
fn codex_handshake_stream_snapshot_and_resume() {
    let mut p = protocol(Kind::Codex);
    p.request.provider_id = Some("saved".into());
    let mut out = Vec::new();
    p.frame(json!({"id":1,"result":{}}), &mut out).unwrap();
    assert!(String::from_utf8_lossy(&out).contains("thread/resume"));
    let resumed: Value = String::from_utf8_lossy(&out)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|message| message["method"] == "thread/resume")
        .unwrap();
    assert_eq!(resumed["params"]["threadId"], "saved");
    assert_eq!(resumed["params"]["excludeTurns"], true);
    p.frame(json!({"id":2,"result":{"thread":{"id":"saved"}}}), &mut out)
        .unwrap();
    let e = p
        .frame(
            json!({"method":"item/agentMessage/delta","params":{"itemId":"a","delta":"Hi\n"}}),
            &mut out,
        )
        .unwrap();
    assert_eq!(e, vec![Event::BeginAssistant, Event::Text("Hi\n".into())]);
    assert_eq!(p.frame(json!({"method":"item/completed","params":{"item":{"type":"agentMessage","id":"a","text":"Hi\n\n"}}}), &mut out).unwrap(),vec![Event::Text("\n".into())]);
    assert_eq!(
        p.frame(
            json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}}),
            &mut out
        )
        .unwrap(),
        vec![Event::Done]
    );
}
#[test]
fn claude_stream_and_completed_snapshot_do_not_duplicate() {
    let mut p = protocol(Kind::Claude);
    let mut out = Vec::new();
    p.frame(
        json!({"type":"stream_event","event":{"type":"message_start"}}),
        &mut out,
    )
    .unwrap();
    p.frame(json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello 🙂"}}}), &mut out).unwrap();
    assert!(p
        .frame(
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"Hello 🙂"}]}}),
            &mut out
        )
        .unwrap()
        .is_empty());
    assert!(p
        .frame(
            json!({"type":"result","is_error":true,"result":"Login required"}),
            &mut out
        )
        .unwrap_err()
        .contains("Login"));
}
#[test]
fn unknown_server_requests_are_resolved_and_errors_surface() {
    let mut p = protocol(Kind::Codex);
    let mut out = Vec::new();
    p.frame(
        json!({"id":"req-1","method":"unsupported/request","params":{}}),
        &mut out,
    )
    .unwrap();
    assert!(String::from_utf8_lossy(&out).contains("req-1"));
    assert!(p
        .frame(
            json!({"id":2,"error":{"message":"Resume failed"}}),
            &mut out
        )
        .unwrap_err()
        .contains("Resume failed"));
}
#[test]
fn approvals_wait_for_a_decision_and_keep_the_requested_input() {
    let mut codex = protocol(Kind::Codex);
    let mut out = Vec::new();
    let events = codex.frame(json!({"id":"permission-1","method":"item/commandExecution/requestApproval","params":{"command":"cargo build","itemId":"c1"}}), &mut out).unwrap();
    let Event::Approval(approval) = &events[0] else {
        panic!("expected an approval")
    };
    assert!(
        out.is_empty(),
        "never answer an approval before a native decision"
    );
    codex
        .respond(
            Control {
                id: approval.id.clone(),
                decision: Decision::Deny,
            },
            &mut out,
        )
        .unwrap();
    let response: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(response["id"], "permission-1");
    assert_eq!(response["result"]["decision"], "decline");
    let mut claude = protocol(Kind::Claude);
    out.clear();
    let input = json!({"file_path":"/work/example.rs","content":"approved"});
    let events = claude.frame(json!({"type":"control_request","request_id":"permission-2","request":{"subtype":"can_use_tool","tool_name":"Write","input":input}}), &mut out).unwrap();
    let Event::Approval(approval) = &events[0] else {
        panic!("expected an approval")
    };
    assert!(out.is_empty());
    claude
        .respond(
            Control {
                id: approval.id.clone(),
                decision: Decision::Allow,
            },
            &mut out,
        )
        .unwrap();
    let response: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(response["response"]["request_id"], "permission-2");
    assert_eq!(response["response"]["response"]["behavior"], "allow");
    assert_eq!(response["response"]["response"]["updatedInput"], input);
}
#[test]
fn selected_model_effort_and_access_reach_the_codex_turn() {
    let mut p = protocol(Kind::Codex);
    p.request.model = Some("selected-model".into());
    p.request.settings.effort = Some("high".into());
    p.request.settings.access = AccessMode::AutoAcceptEdits;
    let mut out = Vec::new();
    p.frame(json!({"id":1,"result":{}}), &mut out).unwrap();
    let rows: Vec<Value> = String::from_utf8_lossy(&out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows[1]["params"]["sandbox"], "workspace-write");
    out.clear();
    p.frame(
        json!({"id":2,"result":{"thread":{"id":"thread"},"model":"selected-model"}}),
        &mut out,
    )
    .unwrap();
    let row: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(row["params"]["model"], "selected-model");
    assert_eq!(row["params"]["effort"], "high");
    assert_eq!(row["params"]["sandboxPolicy"]["type"], "workspaceWrite");
    assert_eq!(row["params"]["approvalsReviewer"], "user");
}
#[test]
fn codex_tool_deltas_settle_to_one_completed_snapshot() {
    let mut p = protocol(Kind::Codex);
    let mut out = Vec::new();
    let started = p.frame(json!({"method":"item/started","params":{"item":{"id":"cmd-1","type":"commandExecution","command":"cargo check","status":"inProgress"}}}), &mut out).unwrap();
    assert!(
        matches!(&started[0], Event::Tool(t) if t.state == ToolState::Running && t.summary() == ("Run".into(), "cargo check".into()))
    );
    p.frame(json!({"method":"item/commandExecution/outputDelta","params":{"itemId":"cmd-1","delta":"Checking 🙂"}}), &mut out).unwrap();
    let completed = p.frame(json!({"method":"item/completed","params":{"item":{"id":"cmd-1","type":"commandExecution","command":"cargo check","status":"completed","aggregatedOutput":"Checking 🙂\nFinished"}}}), &mut out).unwrap();
    assert!(
        matches!(&completed[0], Event::Tool(t) if t.id == "cmd-1" && t.state == ToolState::Completed && t.output == "Checking 🙂\nFinished")
    );
    assert_eq!(p.tools.len(), 1);
}
#[test]
fn codex_live_tool_preserves_argv_and_semantic_rollout_metadata() {
    let mut protocol = protocol(Kind::Codex);
    let mut out = Vec::new();
    let command = json!(["/bin/zsh", "-lc", "sed -n '8,20p' 'src/my file.rs'"]);
    let events = protocol.frame(json!({"method":"item/started","params":{"item":{"id":"read","type":"commandExecution","command":command,"parsed_cmd":[{"type":"read","path":"/repo/src/my file.rs"}],"cwd":"/repo","status":"inProgress"}}}), &mut out).unwrap();
    let Event::Tool(call) = &events[0] else {
        panic!()
    };
    assert_eq!(call.summary(), ("Read".into(), "src/my file.rs".into()));
    assert_eq!(
        call.presentation().command.as_deref(),
        Some("sed -n '8,20p' 'src/my file.rs'")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&call.input).unwrap()["command"],
        command
    );
    let events = protocol.frame(json!({"method":"item/completed","params":{"item":{"id":"read","type":"commandExecution","status":"failed","aggregatedOutput":"permission denied","exitCode":1}}}), &mut out).unwrap();
    let Event::Tool(call) = &events[0] else {
        panic!()
    };
    assert_eq!(call.summary().0, "Read");
    assert_eq!(call.output, "permission denied");
    assert_eq!(call.state, ToolState::Failed);
    assert_eq!(
        serde_json::from_str::<Value>(&call.input).unwrap()["command"],
        command
    );
}
#[test]
fn claude_tool_input_and_result_keep_identity_and_failure() {
    let mut p = protocol(Kind::Claude);
    let mut out = Vec::new();
    p.frame(json!({"type":"stream_event","event":{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"read-1","name":"Read"}}}), &mut out).unwrap();
    p.frame(json!({"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"src/main.rs\"}"}}}), &mut out).unwrap();
    let input = p.tools["read-1"].summary();
    assert_eq!(input, ("Read".into(), "src/main.rs".into()));
    p.frame(json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"read-1","name":"Read","input":{"file_path":"src/main.rs"}}]}}), &mut out).unwrap();
    let events = p.frame(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"read-1","is_error":true,"content":[{"type":"text","text":"File missing"}]}]}}), &mut out).unwrap();
    assert!(
        matches!(&events[0], Event::Tool(t) if t.id == "read-1" && t.state == ToolState::Failed && t.output == "File missing")
    );
    assert_eq!(p.tools.len(), 1);
}
#[test]
fn claude_partial_tool_input_waits_until_it_parses() {
    let mut p = protocol(Kind::Claude);
    let mut out = Vec::new();
    p.frame(json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"sh","name":"Bash"}}}), &mut out).unwrap();
    let events = p.frame(json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\": \"git status"}}}), &mut out).unwrap();
    assert!(events.is_empty());
    assert_eq!(p.tools["sh"].input, "");
    let events = p.frame(json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":" --short\"}"}}}), &mut out).unwrap();
    let Event::Tool(call) = &events[0] else {
        panic!()
    };
    assert_eq!(call.input, r#"{"command": "git status --short"}"#);
}
