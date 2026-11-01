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
fn tool(events: &[Event]) -> &ToolCall {
    events
        .iter()
        .rev()
        .find_map(|e| {
            if let Event::Tool(tool) = e {
                Some(tool)
            } else {
                None
            }
        })
        .unwrap()
}
fn spawn(p: &mut Protocol) {
    p.frame(json!({"method":"item/completed","params":{"threadId":"main","item":{"id":"spawn-id","type":"collabAgentToolCall","tool":"spawnAgent","status":"completed","receiverThreadIds":["child"],"prompt":"Audit correctness","model":"child-model"}}}), &mut Vec::new()).unwrap();
}
#[test]
fn codex_path_titles_match_tauri_for_both_item_formats() {
    for kind in ["collabAgentToolCall", "subAgentActivity"] {
        for (key, path, expected) in [
            ("agentPath", "/root/explore-auth", "Explore Auth subagent"),
            (
                "agent_path",
                "\\root\\pairing__dialog-parity\\",
                "Pairing Dialog Parity subagent",
            ),
        ] {
            let mut p = protocol(Kind::Codex);
            let mut item = json!({
                "id":"spawn-id", "type":kind, "tool":"spawnAgent", "kind":"started",
                "receiverThreadIds":["child"], "agentThreadId":"child",
                "prompt":"Full instructions for the delegated task.", "model":"child-model"
            });
            item[key] = json!(path);
            let events = p
                .frame(
                    json!({"method":"item/completed","params":{"threadId":"main","item":item}}),
                    &mut Vec::new(),
                )
                .unwrap();
            let Some(Panel::Agent(run)) = tool(&events).panel() else {
                panic!()
            };
            assert_eq!(run.display_name(), expected);
            assert_eq!(run.display_brief(), expected);
            assert_eq!(run.model.as_deref(), Some("child-model"));
            assert_eq!(tool(&events).state, ToolState::Running);

            // Sparse completion and child work must retain the title.
            let events = p
                .frame(
                    json!({"method":"item/completed","params":{"threadId":"main","item":{
                        "id":"spawn-id", "type":kind, "tool":"spawnAgent", "kind":"interacted",
                        "receiverThreadIds":["child"], "agentThreadId":"child"
                    }}}),
                    &mut Vec::new(),
                )
                .unwrap();
            let Some(Panel::Agent(run)) = tool(&events).panel() else {
                panic!()
            };
            assert_eq!(run.display_name(), expected);
            let events = p.frame(
                    json!({"method":"item/completed","params":{"threadId":"child","item":{
                        "id":"read", "type":"commandExecution", "command":"cat source.rs", "status":"completed"
                    }}}), &mut Vec::new(),
                ).unwrap();
            let Some(Panel::Agent(run)) = tool(&events).panel() else {
                panic!()
            };
            assert_eq!(run.display_name(), expected);
            assert_eq!(run.steps.len(), 1);
            // Desktop persistence carries the same label back to native.
            let restored = crate::tool::tasks::from_desktop(&json!({
                "role":"tool", "agentRun":crate::tool::tasks::desktop_agent(&run)
            }))
            .unwrap();
            assert_eq!(restored, Panel::Agent(run));
        }
    }
}
#[test]
fn codex_duplicate_path_items_keep_the_original_task_title() {
    let mut p = protocol(Kind::Codex);
    spawn(&mut p);
    let events = p
        .frame(
            json!({"method":"item/completed","params":{"threadId":"main","item":{
                "id":"activity-id", "type":"subAgentActivity", "kind":"started",
                "agentPath":"/root/explore-auth", "agentThreadId":"child"
            }}}),
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(tool(&events).id, "spawn-id");
    let Some(Panel::Agent(run)) = tool(&events).panel() else {
        panic!()
    };
    assert_eq!(run.display_name(), "Audit correctness");
    assert!(!p.tools.contains_key("activity-id"));
}
#[test]
fn codex_completed_spawn_wait_and_child_lifecycle_keep_actual_owner() {
    let mut p = protocol(Kind::Codex);
    spawn(&mut p);
    assert_eq!(p.tools["spawn-id"].state, ToolState::Running);
    p.active_turn = Some("parent-turn".into());
    let child = p.frame(json!({"method":"item/completed","params":{"threadId":"child","item":{"id":"child-answer","type":"agentMessage","text":"Only the child said this"}}}), &mut Vec::new()).unwrap();
    assert_eq!(child.len(), 1);
    assert_eq!(tool(&child).id, "spawn-id");
    let Some(Panel::Agent(run)) = tool(&child).panel() else {
        panic!()
    };
    assert_eq!(run.ids, vec!["child"]);
    assert_eq!(run.display_name(), "Audit correctness");
    assert_eq!(run.model.as_deref(), Some("child-model"));
    assert_eq!(run.steps[0].id, "child-answer");
    assert_eq!(run.steps[0].text, "Only the child said this");
    let completed = p.frame(json!({"method":"turn/completed","params":{"threadId":"child","turn":{"id":"child-turn","status":"completed"}}}), &mut Vec::new()).unwrap();
    assert_eq!(tool(&completed).state, ToolState::Completed);
    assert_eq!(p.active_turn.as_deref(), Some("parent-turn"));
    assert!(!p.done);
    let settled = p.frame(json!({"method":"item/completed","params":{"threadId":"main","item":{"id":"wait-id","type":"collabAgentToolCall","tool":"wait","agentsStates":{"child":{"status":"failed","message":"Provider failure"}}}}}), &mut Vec::new()).unwrap();
    assert_eq!(tool(&settled).id, "spawn-id");
    assert_eq!(tool(&settled).state, ToolState::Failed);
    assert_eq!(tool(&settled).output, "Provider failure");
    assert!(!p.tools.contains_key("wait-id"));
}
#[test]
fn codex_early_child_snapshots_replay_and_do_not_finish_main_turn() {
    let mut p = protocol(Kind::Codex);
    p.activities.main_thread = Some("main".into());
    let events = p.frame(json!({"method":"item/completed","params":{"threadId":"child","item":{"id":"early","type":"agentMessage","text":"Early snapshot"}}}), &mut Vec::new()).unwrap();
    assert!(events.is_empty());
    spawn(&mut p);
    let Some(Panel::Agent(run)) = p.tools["spawn-id"].panel() else {
        panic!()
    };
    assert_eq!(run.steps[0].id, "early");
    assert!(p.activities.pending.is_empty());
    let approval = p.frame(json!({"id":88,"method":"item/commandExecution/requestApproval","params":{"threadId":"child","command":"write"}}), &mut Vec::new()).unwrap();
    assert!(approval
        .iter()
        .any(|event| matches!(event, Event::Approval(_))));
}
#[test]
fn codex_plan_snapshots_update_one_real_turn_and_clear_explicitly() {
    let mut p = protocol(Kind::Codex);
    let a = p.frame(json!({"method":"turn/plan/updated","params":{"turnId":"real-turn","explanation":"Inspect first","plan":[{"step":"Inspect","status":"in_progress"}]}}), &mut Vec::new()).unwrap();
    assert_eq!(tool(&a).id, "plan:real-turn");
    let b = p
        .frame(
            json!({"method":"turn/plan/updated","params":{"turnId":"real-turn","plan":[]}}),
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(tool(&a).id, tool(&b).id);
    let Some(Panel::Tasks(list)) = tool(&b).panel() else {
        panic!()
    };
    assert!(list.items.is_empty());
}
#[test]
fn claude_child_work_never_leaks_into_parent_answer_or_metrics() {
    let mut p = protocol(Kind::Claude);
    p.frame(json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"agent-call","name":"Agent","input":{"description":"Correctness review","model":"requested"}}]}}), &mut Vec::new()).unwrap();
    let child = json!({"type":"assistant","parent_tool_use_id":"agent-call","message":{"id":"child-message","model":"actual-child-model","usage":{"input_tokens":999999},"content":[{"type":"text","text":"Child narration"},{"type":"tool_use","id":"child-tool","name":"Read","input":{"file_path":"source.rs"}}]}});
    let events = p.frame(child.clone(), &mut Vec::new()).unwrap();
    assert!(events.iter().all(|event| matches!(event, Event::Tool(_))));
    let Some(Panel::Agent(run)) = tool(&events).panel() else {
        panic!()
    };
    assert_eq!(run.model.as_deref(), Some("actual-child-model"));
    assert_eq!(run.steps.len(), 2);
    assert_eq!(run.steps[0].id, "child-message:0");
    p.frame(json!({"type":"user","parent_tool_use_id":"agent-call","message":{"content":[{"type":"tool_result","tool_use_id":"child-tool","content":"Read result"}]}}), &mut Vec::new()).unwrap();
    p.frame(child, &mut Vec::new()).unwrap();
    let Some(Panel::Agent(run)) = p.tools["agent-call"].panel() else {
        panic!()
    };
    assert_eq!(run.steps.len(), 2);
    assert_eq!(run.steps[1].state, Some(ToolState::Completed));
    assert_eq!(run.steps[1].detail, "Read result");
    assert!(p.tools["agent-call"].output.is_empty());
}
#[test]
fn claude_task_descriptions_keep_the_name_after_child_work_starts() {
    let mut p = protocol(Kind::Claude);
    p.frame(
        json!({"type":"assistant","message":{"content":[{
            "type":"tool_use","id":"agent-call","name":"Agent",
            "input":{"description":"Review implementation"}
        }]}}),
        &mut Vec::new(),
    )
    .unwrap();
    // Task metadata can refine the name before the child starts working.
    p.frame(
        json!({"type":"system","subtype":"task_started","task_id":"review-task",
                "tool_use_id":"agent-call","description":"Correctness review"}),
        &mut Vec::new(),
    )
    .unwrap();
    p.frame(
        json!({"type":"assistant","parent_tool_use_id":"agent-call",
        "message":{"id":"child-message","content":[{
            "type":"tool_use","id":"child-read","name":"Read",
            "input":{"file_path":"source.rs"}
        }]}}),
        &mut Vec::new(),
    )
    .unwrap();

    for update in [
        json!({"type":"system","subtype":"task_progress","task_id":"review-task",
                "description":"Running cd /repo && cargo check",
                "summary":"Inspecting files"}),
        json!({"type":"system","subtype":"task_updated","task_id":"review-task",
                "patch":{"description":"Running Read phases."}}),
        json!({"type":"system","subtype":"task_notification","task_id":"review-task",
                "description":"Finished reviewing source.rs","status":"completed",
                "summary":"No regressions found."}),
    ] {
        let events = p.frame(update, &mut Vec::new()).unwrap();
        let call = tool(&events);
        let Some(Panel::Agent(run)) = call.panel() else {
            panic!()
        };
        assert_eq!(run.display_name(), "Correctness review");
        assert_eq!(run.display_brief(), "Correctness review");
        assert_eq!(run.status_line(&call.state), "1 step");
        assert!(run.steps.iter().any(|step| step.text == "Inspecting files"));
    }
    assert_eq!(p.tools["agent-call"].state, ToolState::Completed);
    assert_eq!(p.tools["agent-call"].output, "No regressions found.");
}
#[test]
fn claude_task_ids_progress_and_reports_come_from_task_events() {
    let mut p = protocol(Kind::Claude);
    p.tools
        .insert("agent".into(), ToolCall::new("agent", "Agent"));
    p.frame(json!({"type":"system","subtype":"task_started","task_id":"actual-task","tool_use_id":"agent","description":"Review","is_backgrounded":true}), &mut Vec::new()).unwrap();
    p.frame(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"agent","content":"Background launch receipt"}]}}), &mut Vec::new()).unwrap();
    assert_eq!(p.tools["agent"].state, ToolState::Running);
    assert!(p.tools["agent"].output.is_empty());
    let progress = p.frame(json!({"type":"system","subtype":"task_progress","task_id":"actual-task","summary":"Inspecting files"}), &mut Vec::new()).unwrap();
    let Some(Panel::Agent(run)) = tool(&progress).panel() else {
        panic!()
    };
    assert_eq!(run.ids, vec!["actual-task"]);
    assert_eq!(run.steps[0].text, "Inspecting files");
    assert!(tool(&progress).output.is_empty());
    let done = p.frame(json!({"type":"system","subtype":"task_notification","task_id":"actual-task","status":"failed","summary":"Provider reported failure"}), &mut Vec::new()).unwrap();
    assert_eq!(tool(&done).state, ToolState::Failed);
    assert_eq!(tool(&done).output, "Provider reported failure");
    let orphan = p.frame(json!({"type":"assistant","parent_tool_use_id":"unknown","message":{"id":"x","content":[{"type":"text","text":"Unattributed child"}]}}), &mut Vec::new()).unwrap();
    assert!(orphan.is_empty());
}
