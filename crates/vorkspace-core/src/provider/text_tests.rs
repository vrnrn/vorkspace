use super::*;
use std::os::unix::fs::PermissionsExt;

fn fixture(mode: &str) -> (tempfile::TempDir, Request) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider");
    let script = format!(
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys, time
mode = {mode:?}
restricted = "--strict-config" in sys.argv
verified = False
def send(value):
    print(json.dumps(value), flush=True)
for line in sys.stdin:
    if restricted and 'transcript fixture' in line:
        pathlib.Path('transcript-sent').touch()
    value = json.loads(line)
    if value.get('id') == 1:
        version = '0.165.0' if mode == 'old' else '0.166.0'
        send({{'id':1,'result':{{'userAgent':'vorkspace Native/' + version}}}})
    elif value.get('id') == 4:
        assert restricted and value['method'] == 'config/read'
        flags = {{name:False for option,name in zip(sys.argv,sys.argv[1:]) if option == '--disable'}}
        assert flags['shell_tool'] is False and flags['hooks'] is False and flags['apps'] is False
        assert 'agents.enabled=false' in sys.argv
        assert pathlib.Path(os.environ['CODEX_HOME'], 'selected-account').is_file()
        assert os.getenv('OPENAI_API_KEY') is None
        assert os.getenv('VORKSPACE_TEST_CAPABILITY') is None
        if mode == 'unsafe-config':
            flags['hooks'] = True
        send({{'id':4,'result':{{'config':{{'features':flags,'web_search':'disabled','notify':[],'agents':{{'enabled':False}},
                'mcp_servers':{{'configured-mcp':{{'enabled':True}}}}}}}}}})
    elif value.get('id') == 2:
        assert value['method'] == 'thread/start'
        assert value['params']['ephemeral'] is True
        assert value['params']['sandbox'] == 'read-only'
        assert value['params']['approvalPolicy'] == 'never'
        if restricted:
            assert mode not in ('old', 'unsafe-config')
            assert value['params']['environments'] == []
            assert value['params']['model'] == 'summary-model'
            assert value['params']['config']['mcp_servers'] == {{'configured-mcp':{{'enabled':False}}}}
            assert 'Return only the finished meeting-note content' in value['params']['baseInstructions']
            assert 'Omit model reasoning, work steps, and progress commentary' in value['params']['developerInstructions']
            assert 'transcript fixture' not in line
        else:
            assert 'Do not call tools' in value['params']['developerInstructions']
        assert os.getenv('VORKSPACE_TEST_CAPABILITY') is None
        send({{'id':2,'result':{{'thread':{{'id':'fresh-text-thread','ephemeral':True}},
            'approvalPolicy':'never','sandbox':{{'type':'readOnly','networkAccess':mode == 'unsafe-thread'}}}}}})
    elif value.get('id') == 5:
        assert restricted and value['method'] == 'mcpServerStatus/list'
        assert value['params']['threadId'] == 'fresh-text-thread'
        assert 'transcript fixture' not in line
        if mode == 'mcp-unavailable':
            send({{'id':5,'error':{{'code':-32601,'message':'No such method'}}}})
        else:
            verified = mode != 'unsafe-mcp'
            status = 'disabled' if verified else 'ready'
            send({{'id':5,'result':{{'data':[{{'runtimeStatus':status,'tools':{{}}}}],'nextCursor':None}}}})
    elif value.get('id') == 3:
        if restricted:
            assert verified and 'transcript fixture' in line
        assert value['params']['threadId'] == 'fresh-text-thread'
        assert len(value['params']['input']) == 1
        if mode == 'cancel':
            time.sleep(10)
        elif mode == 'slow' or mode.startswith('summary'):
            if mode == 'slow':
                time.sleep(0.25)
            send({{'method':'item/agentMessage/delta','params':{{'itemId':'progress','delta':'I will prepare the notes.'}}}})
            send({{'method':'item/completed','params':{{'item':{{'type':'agentMessage','id':'progress','phase':'commentary','text':'I will prepare the notes.'}}}}}})
            send({{'method':'item/reasoning/textDelta','params':{{'itemId':'reasoning','delta':'Private model reasoning'}}}})
            if mode == 'summary-unphased':
                send({{'method':'item/completed','params':{{'item':{{'type':'agentMessage','id':'legacy-progress','phase':None,'text':'Earlier unknown-phase message'}}}}}})
            if mode == 'summary-foreign':
                send({{'method':'item/completed','params':{{'threadId':'foreign-thread','item':{{'type':'agentMessage','id':'foreign-answer','phase':'final_answer','text':'Foreign answer'}}}}}})
                send({{'method':'turn/completed','params':{{'threadId':'foreign-thread','turn':{{'status':'completed'}}}}}})
            if mode not in ('summary-missing', 'summary-foreign'):
                text = '  ' if mode == 'summary-empty' else 'Meeting summary'
                phase = None if mode == 'summary-unphased' else 'final_answer'
                send({{'method':'item/agentMessage/delta','params':{{'itemId':'answer','delta':text}}}})
                send({{'method':'item/completed','params':{{'item':{{'type':'agentMessage','id':'answer','phase':phase,'text':text}}}}}})
            if mode == 'summary-error':
                send({{'method':'error','params':{{'willRetry':False,'error':{{'message':'Stream failed after final'}}}}}})
            else:
                status = 'failed' if mode == 'summary-failed' else 'interrupted' if mode == 'summary-interrupted' else 'completed'
                send({{'method':'turn/completed','params':{{'turn':{{'status':status,'error':{{'message':'Turn failed after final'}} if status == 'failed' else None}}}}}})
        elif mode == 'tool':
            send({{'method':'item/started','params':{{'item':{{'type':'commandExecution','id':'tool','command':'never execute'}}}}}})
        else:
            send({{'method':'item/agentMessage/delta','params':{{'itemId':'answer','delta':'{{"title":"Improve session filters","workItem":null}}'}}}})
            send({{'method':'turn/completed','params':{{'turn':{{'status':'completed'}}}}}})
"#
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let request = Request {
        kind: Kind::Codex,
        binary: path,
        cwd: dir.path().into(),
        prompt: "Generate a session title".into(),
        provider_id: Some("user-thread-must-not-be-used".into()),
        model: None,
        settings: Settings {
            access: AccessMode::FullAccess,
            environment: [("VORKSPACE_TEST_CAPABILITY".into(), "private".into())].into(),
            attachments: vec![dir.path().join("private-file")],
            ..Default::default()
        },
    };
    (dir, request)
}

#[test]
fn background_title_uses_an_isolated_read_only_thread_and_strips_capabilities() {
    let (_dir, request) = fixture("title");
    let output = run_text(request, Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(
        crate::session_title::parse(&output, "").unwrap().title,
        "Improve session filters"
    );
}

#[test]
fn background_titles_reject_tools_and_reap_on_cancellation() {
    let (_dir, request) = fixture("tool");
    assert!(run_text(request, Arc::new(AtomicBool::new(false)))
        .unwrap_err()
        .contains("attempted to use tools"));
    let (_dir, request) = fixture("cancel");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let task = thread::spawn(move || run_text(request, flag));
    thread::sleep(Duration::from_millis(150));
    cancel.store(true, Ordering::Relaxed);
    assert!(task.join().unwrap().unwrap_err().contains("Stopped"));
}

#[test]
fn caller_controls_the_background_text_timeout() {
    let (_dir, request) = meeting_fixture("slow");
    let error = run_restricted_text(
        request,
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(10),
    )
    .unwrap_err();
    assert_eq!(error, "Background text request timed out");

    let (_dir, request) = meeting_fixture("slow");
    assert_eq!(
        run_restricted_text(
            request,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5)
        )
        .unwrap(),
        "Meeting summary"
    );
}

fn meeting_fixture(mode: &str) -> (tempfile::TempDir, Request) {
    let (dir, mut request) = fixture(mode);
    request.prompt = "transcript fixture".into();
    request.model = Some("summary-model".into());
    request.settings.account = crate::accounts::Profile::new("meeting-account", dir.path());
    let home = request.settings.account.home(Kind::Codex).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("selected-account"), "").unwrap();
    (dir, request)
}

#[test]
fn codex_summary_sends_transcript_only_after_verifying_the_selected_accounts_thread() {
    let (dir, request) = meeting_fixture("summary");
    let output = run_restricted_text(
        request,
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(output, "Meeting summary");
    assert!(dir.path().join("transcript-sent").is_file());
}

#[test]
fn codex_summary_accepts_only_the_last_completed_unphased_message_for_legacy_models() {
    let (_dir, request) = meeting_fixture("summary-unphased");
    assert_eq!(
        run_restricted_text(
            request,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5),
        )
        .unwrap(),
        "Meeting summary"
    );
}

#[test]
fn codex_summary_rejects_missing_empty_or_unsuccessful_final_output() {
    for (mode, expected) in [
        ("summary-missing", "no completed final response"),
        ("summary-foreign", "no completed final response"),
        ("summary-empty", "empty final response"),
        ("summary-failed", "Turn failed after final"),
        ("summary-error", "Stream failed after final"),
        ("summary-interrupted", "no completed final response"),
    ] {
        let (_dir, request) = meeting_fixture(mode);
        let error = run_restricted_text(
            request,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(error.contains(expected), "{mode}: {error}");
    }
}

#[test]
fn codex_summary_keeps_transcript_local_when_restrictions_cannot_be_verified() {
    for (mode, expected) in [
        ("old", "require Codex CLI"),
        ("unsafe-config", "could not establish restricted access"),
        ("unsafe-thread", "could not establish restricted access"),
        ("unsafe-mcp", "could not establish restricted access"),
        ("mcp-unavailable", "could not establish restricted access"),
    ] {
        let (dir, request) = meeting_fixture(mode);
        let error = run_restricted_text(
            request,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(error.contains(expected), "{mode}: {error}");
        assert!(!dir.path().join("transcript-sent").exists());
    }
}

fn alternative_meeting_fixture(kind: Kind, fail: bool) -> (tempfile::TempDir, Request) {
    let (dir, mut request) = fixture("unused");
    request.kind = kind;
    request.prompt = "transcript fixture".into();
    let script = format!(
        r#"#!/usr/bin/env python3
import json, sys
kind = {kind:?}
fail = {fail}
assert '--system-prompt' in sys.argv
assert 'Return only the finished meeting-note content' in sys.argv[sys.argv.index('--system-prompt') + 1]
notes = '## Summary\nFinal meeting notes'
def send(value):
    print(json.dumps(value), flush=True)
if kind == 'claude':
    assert sys.argv[sys.argv.index('--tools') + 1] == ''
else:
    assert all(flag in sys.argv for flag in ('--no-session', '--no-tools', '--no-extensions', '--no-skills'))
for line in sys.stdin:
    value = json.loads(line)
    if kind == 'claude':
        if value.get('type') == 'control_request':
            send({{'type':'control_response','response':{{'request_id':'native-init'}}}})
        elif value.get('type') == 'user':
            send({{'type':'stream_event','event':{{'type':'content_block_delta','delta':{{'type':'thinking_delta','thinking':'Private reasoning'}}}}}})
            send({{'type':'assistant','message':{{'content':[{{'type':'text','text':'I will prepare the notes.'}}]}}}})
            send({{'type':'assistant','message':{{'content':[{{'type':'text','text':notes}}]}}}})
            send({{'type':'result','is_error':fail,'result':'Meeting generation failed' if fail else notes}})
    elif value.get('type') == 'get_state':
        send({{'type':'response','id':value['id'],'success':True,'data':{{'sessionId':'isolated-meeting'}}}})
    elif value.get('type') == 'set_fast_mode':
        send({{'type':'response','id':value['id'],'success':True}})
    elif value.get('type') == 'prompt':
        send({{'type':'response','id':value['id'],'command':'prompt','success':True}})
        send({{'type':'message_start','message':{{'role':'assistant'}}}})
        send({{'type':'message_update','assistantMessageEvent':{{'type':'text_delta','delta':'I will prepare the notes.'}}}})
        send({{'type':'message_update','assistantMessageEvent':{{'type':'thinking_delta','delta':'Private reasoning'}}}})
        send({{'type':'message_end','message':{{'role':'assistant','stopReason':'stop','content':[
            {{'type':'thinking','thinking':'Private reasoning'}},{{'type':'text','text':notes}}]}}}})
        if fail:
            send({{'type':'message_end','message':{{'role':'assistant','stopReason':'error','errorMessage':'Meeting generation failed'}}}})
        send({{'type':'agent_end'}})
    elif value.get('type') == 'get_session_stats':
        send({{'type':'response','id':value['id'],'success':True,'data':{{}}}})
"#,
        kind = kind.id(),
        fail = if fail { "True" } else { "False" },
    );
    std::fs::write(&request.binary, script).unwrap();
    (dir, request)
}

#[test]
fn all_alternative_summary_providers_publish_only_a_successful_completed_answer() {
    for kind in [Kind::Claude, Kind::Pi, Kind::Omp] {
        for fail in [false, true] {
            let (_dir, request) = alternative_meeting_fixture(kind, fail);
            let result = run_restricted_text(
                request,
                Arc::new(AtomicBool::new(false)),
                Duration::from_secs(5),
            );
            if fail {
                assert!(
                    result.unwrap_err().contains("Meeting generation failed"),
                    "{}",
                    kind.label()
                );
            } else {
                assert_eq!(result.unwrap(), "## Summary\nFinal meeting notes");
            }
        }
    }
}
