use super::*;
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
fn last(out: &mut Vec<u8>) -> Value {
    let text = String::from_utf8(std::mem::take(out)).unwrap();
    serde_json::from_str(text.lines().last().unwrap()).unwrap()
}
#[test]
fn antigravity_runs_its_native_session_lifecycle() {
    for kind in Kind::ALL.into_iter().filter(|k| k.is_acp()) {
        let mut p = protocol(kind);
        let mut out = Vec::new();
        p.acp_session(&mut out).unwrap();
        let new = last(&mut out);
        assert_eq!(new["method"], "session/new");
        let events = p
            .frame(
                json!({"id":new["id"],"result":{"sessionId":"s","models":{"currentModelId":"m"}}}),
                &mut out,
            )
            .unwrap();
        assert!(events.contains(&Event::SessionId("s".into())));
        loop {
            let request = last(&mut out);
            if request["method"] == "session/prompt" {
                assert_eq!(request["params"]["prompt"][0]["text"], "Hello");
                let live = p.frame(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello"}}}}),&mut out).unwrap();
                assert!(live.contains(&Event::Text("Hello".into())));
                assert_eq!(
                    p.frame(
                        json!({"id":request["id"],"result":{"stopReason":"end_turn"}}),
                        &mut out
                    )
                    .unwrap(),
                    vec![Event::Done]
                );
                break;
            }
            p.frame(json!({"id":request["id"],"result":{}}), &mut out)
                .unwrap();
        }
        assert!(p.done);
    }
}
#[test]
fn resuming_mutes_replay_and_replaces_missing_sessions_like_the_desktop() {
    let mut p = protocol(Kind::Antigravity);
    p.request.provider_id = Some("saved".into());
    let mut out = Vec::new();
    p.acp_session(&mut out).unwrap();
    let request = last(&mut out);
    assert_eq!(request["method"], "session/resume");
    p.frame(
        json!({"id":request["id"],"error":{"code":-32601,"message":"unsupported"}}),
        &mut out,
    )
    .unwrap();
    let request = last(&mut out);
    assert_eq!(request["method"], "session/load");
    assert!(p.frame(json!({"method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"text":"Old history"}}}}),&mut out).unwrap().is_empty());
    p.frame(
        json!({"id":request["id"],"error":{"message":"Session missing"}}),
        &mut out,
    )
    .unwrap();
    let replacement = last(&mut out);
    assert_eq!(replacement["method"], "session/new");
    let events = p
        .frame(
            json!({"id":replacement["id"],"result":{"sessionId":"replacement"}}),
            &mut out,
        )
        .unwrap();
    assert!(events.contains(&Event::SessionId("replacement".into())));
    assert_eq!(p.acp.session, "replacement");
    let mode = last(&mut out);
    assert_eq!(mode["method"], "session/set_mode");
    assert_eq!(mode["params"]["modeId"], "default");
    p.frame(json!({"id":mode["id"],"result":{}}), &mut out)
        .unwrap();
    assert_eq!(last(&mut out)["method"], "session/prompt");
}
#[test]
fn uncertain_resume_or_load_never_starts_a_replacement_session() {
    for load in [false, true] {
        let mut protocol = protocol(Kind::Antigravity);
        protocol.request.provider_id = Some("saved".into());
        let mut out = Vec::new();
        protocol.acp_session(&mut out).unwrap();
        let resume = last(&mut out);
        if load {
            protocol
                .frame(
                    json!({"id":resume["id"],"error":{"message":"Unsupported method"}}),
                    &mut out,
                )
                .unwrap();
            assert_eq!(last(&mut out)["method"], "session/load");
        }
        assert!(protocol
            .acp_failure(&json!({"message":"Request timed out"}), true, &mut out)
            .unwrap_err()
            .contains("Request timed out"));
        assert!(
            out.is_empty(),
            "an uncertain external result cannot authorize a new session"
        );
        assert_eq!(protocol.request.provider_id.as_deref(), Some("saved"));
    }
}
#[test]
fn permissions_use_semantic_kinds_and_never_guess_opaque_allow_options() {
    let params = json!({"options":[{"optionId":"opaque-deny","kind":"reject_once"},{"optionId":"opaque-allow","kind":"allow_once"},{"optionId":"opaque-session","kind":"allow_always"}]});
    for (decision, expected) in [
        (Decision::Allow, "opaque-allow"),
        (Decision::Session, "opaque-session"),
        (Decision::Deny, "opaque-deny"),
    ] {
        let mut out = Vec::new();
        respond(&mut out, json!("permission"), &params, decision).unwrap();
        assert_eq!(last(&mut out)["result"]["outcome"]["optionId"], expected);
    }
    let mut out = Vec::new();
    respond(
        &mut out,
        json!(1),
        &json!({"options":[{"optionId":"unknown"}]}),
        Decision::Allow,
    )
    .unwrap();
    assert_eq!(last(&mut out)["result"]["outcome"]["outcome"], "cancelled");
}
#[test]
fn tool_updates_preserve_identity_input_and_partial_results() {
    let mut p = protocol(Kind::Antigravity);
    p.acp.stage = Stage::Prompt;
    p.acp.session = "s".into();
    for update in [
        json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"Read","kind":"read","rawInput":{"path":"file.rs"}}),
        json!({"sessionUpdate":"tool_call_update","toolCallId":"t","content":[{"type":"content","content":{"type":"text","text":"contents"}}]}),
        json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"completed"}),
    ] {
        p.frame(
            json!({"method":"session/update","params":{"sessionId":"s","update":update}}),
            &mut Vec::new(),
        )
        .unwrap();
    }
    let tool = &p.tools["t"];
    assert_eq!(tool.name, "Read");
    assert!(tool.input.contains("file.rs"));
    assert_eq!(tool.output, "contents");
    assert_eq!(tool.state, ToolState::Completed);
}
#[test]
fn live_acp_tool_keeps_semantic_kind_locations_and_original_input() {
    let mut protocol = protocol(Kind::Antigravity);
    let raw = json!({"command":"/bin/zsh -lc 'cat src/main.rs'","future":true});
    protocol.acp_update(&json!({"sessionUpdate":"tool_call","toolCallId":"shell","kind":"execute","title":"Inspect project","rawInput":raw}), &mut Vec::new());
    let call = &protocol.tools["shell"];
    assert_eq!(call.summary(), ("Read".into(), "src/main.rs".into()));
    assert_eq!(
        serde_json::from_str::<Value>(&call.input_detail()).unwrap(),
        raw
    );
    protocol.acp_update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"shell","status":"completed","content":{"text":"contents"}}), &mut Vec::new());
    let call = &protocol.tools["shell"];
    assert_eq!(call.summary().0, "Read");
    assert_eq!(call.output, "contents");
    assert_eq!(
        serde_json::from_str::<Value>(&call.input_detail()).unwrap(),
        raw
    );
    protocol.acp_update(&json!({"sessionUpdate":"tool_call","toolCallId":"file","kind":"read","title":"Inspect file","locations":[{"path":"src/lib.rs","line":18}]}), &mut Vec::new());
    let shown = protocol.tools["file"].presentation();
    assert_eq!(
        (shown.action.as_str(), shown.target.as_str()),
        ("Read", "src/lib.rs")
    );
    assert_eq!(shown.start_line, Some(18));
}
#[test]
fn planning_denies_execution_even_in_full_access() {
    let mut p = protocol(Kind::Antigravity);
    p.acp.stage = Stage::Prompt;
    p.request.settings.plan = true;
    p.request.settings.access = AccessMode::FullAccess;
    let mut out = Vec::new();
    let events = p.frame(json!({"id":"permission","method":"session/request_permission","params":{"toolCall":{"kind":"execute"},"options":[{"kind":"allow_once","optionId":"yes"},{"kind":"reject_once","optionId":"no"}]}}),&mut out).unwrap();
    assert!(events.is_empty());
    assert_eq!(last(&mut out)["result"]["outcome"]["optionId"], "no");
}
#[test]
fn sparse_permission_requests_keep_the_tool_preview() {
    let mut p = protocol(Kind::Antigravity);
    p.acp.stage = Stage::Prompt;
    let mut tool = ToolCall::new("t", "Write example.rs");
    tool.input = "{\"path\":\"example.rs\"}".into();
    tool.output = "+ new content".into();
    p.tools.insert("t".into(), tool);
    p.acp.tool_kinds.insert("t".into(), "edit".into());
    let events = p.frame(json!({"id":99,"method":"session/request_permission","params":{"toolCall":{"toolCallId":"t"},"options":[{"kind":"allow_once","optionId":"yes"},{"kind":"reject_once","optionId":"no"}]}}),&mut Vec::new()).unwrap();
    let Event::Approval(request) = &events[0] else {
        panic!("expected approval")
    };
    assert_eq!(request.title, "Allow file changes?");
    assert!(request.detail.contains("example.rs") && request.detail.contains("+ new content"));
}
#[test]
fn usage_preserves_context_and_cache_metrics() {
    let mut events = Vec::new();
    usage_events(
        &json!({"usage":{"inputTokens":100,"outputTokens":50,"cacheReadTokens":900,"totalTokens":150,"contextWindow":200000}}),
        &mut events,
    );
    assert!(events.contains(&Event::Context {
        used: Some(150),
        window: Some(200000)
    }));
    assert!(events.iter().any(|event| matches!(event, Event::Metrics(metrics) if metrics.input_tokens == 100 && metrics.output_tokens == 50 && metrics.cache_read_tokens == 900 && metrics.cache_hit_percent == Some(90.))));
}
#[test]
fn controls_use_the_options_advertised_after_model_selection() {
    let mut p = protocol(Kind::Antigravity);
    p.request.model = Some("chosen".into());
    p.request.settings.effort = Some("high".into());
    p.request
        .settings
        .model_settings
        .insert("context".into(), "1m".into());
    p.request
        .settings
        .model_settings
        .insert("unsupported".into(), "ignored".into());
    let mut out = Vec::new();
    p.acp_session(&mut out).unwrap();
    let new = last(&mut out);
    p.frame(json!({"id":new["id"],"result":{"sessionId":"s","configOptions":[{"id":"model","category":"model"}]}}),&mut out).unwrap();
    let select = last(&mut out);
    assert_eq!(select["params"]["value"], "chosen");
    p.frame(json!({"id":select["id"],"result":{"configOptions":[{"id":"effort","category":"thought_level"},{"id":"fast","type":"boolean"},{"id":"context"}]}}),&mut out).unwrap();
    let mut choices = Vec::new();
    loop {
        let request = last(&mut out);
        if request["method"] == "session/prompt" {
            break;
        }
        choices.push(request["params"].clone());
        p.frame(json!({"id":request["id"],"result":{}}), &mut out)
            .unwrap();
    }
    assert_eq!(choices.len(), 4);
    assert!(choices.iter().any(|params| params["modeId"] == "default"));
    assert!(choices
        .iter()
        .any(|p| p["configId"] == "context" && p["value"] == "1m"));
    assert!(choices
        .iter()
        .any(|p| p["configId"] == "effort" && p["value"] == "high"));
    assert!(choices
        .iter()
        .any(|p| p["configId"] == "fast" && p["type"] == "boolean" && p["value"] == false));
}
#[test]
fn antigravity_auto_edit_only_approves_edits_and_read_only_denies_execution() {
    for (access, tool, expected_approval) in [
        (AccessMode::AutoAcceptEdits, "edit", true),
        (AccessMode::AutoAcceptEdits, "execute", false),
        (AccessMode::ReadOnly, "execute", false),
    ] {
        let mut protocol = protocol(Kind::Antigravity);
        protocol.request.settings.access = access;
        protocol.acp.stage = Stage::Prompt;
        let mut out = Vec::new();
        let events = protocol.frame(json!({"id":"permission","method":"session/request_permission","params":{"toolCall":{"kind":tool},"options":[{"kind":"allow_once","optionId":"yes"},{"kind":"reject_once","optionId":"no"}]}}), &mut out).unwrap();
        if access == AccessMode::ReadOnly || expected_approval {
            assert!(events.is_empty());
            assert_eq!(
                last(&mut out)["result"]["outcome"]["optionId"],
                if expected_approval { "yes" } else { "no" }
            );
        } else {
            assert!(matches!(events.as_slice(), [Event::Approval(_)]));
            assert!(out.is_empty());
        }
    }
}
