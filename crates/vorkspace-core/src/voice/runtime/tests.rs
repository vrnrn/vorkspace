use super::*;

#[test]
fn tools_require_exact_scope_known_action_and_bounded_object_input() {
    let valid = json!({"threadId":"mine","tool":"vorkspace","callId":"call-1","arguments":{"action":"composer.write","input":{"text":"draft"},"expression":"happy"}});
    assert!(validate_tool(&valid, Some("mine")).is_ok());
    assert!(validate_tool(&valid, None).is_err());
    assert!(validate_tool(&valid, Some("another")).is_err());
    for (key, value) in [
        ("threadId", json!("another")),
        ("tool", json!("shell")),
        ("namespace", json!("remote")),
        ("callId", json!("")),
        ("callId", json!("x".repeat(129))),
    ] {
        let mut bad = valid.clone();
        bad[key] = value;
        assert!(validate_tool(&bad, Some("mine")).is_err(), "{key}");
    }
    for (key, value) in [
        ("action", json!("shell.exec")),
        ("expression", json!("error")),
        ("input", json!("not an object")),
        ("input", json!({"text":"x".repeat(64000)})),
    ] {
        let mut bad = valid.clone();
        bad["arguments"][key] = value;
        assert!(validate_tool(&bad, Some("mine")).is_err(), "{key}");
    }
}

#[test]
fn catalog_projects_only_bounded_identity_and_names() {
    let bots: Vec<_> = (0..1000)
        .map(|i| json!({"id":i.to_string(),"name":"🦀".repeat(500),"secret":"hidden"}))
        .collect();
    let result = bounded_bots(&json!({"bots":bots}));
    assert!(result.as_array().unwrap().len() <= 64);
    assert!(result.to_string().len() < 10000);
    assert!(!result.to_string().contains("hidden"));
    assert!(result[0]["name"].as_str().unwrap().chars().count() <= 256);
}

#[cfg(unix)]
mod lifecycle {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    // A protocol peer, rather than a mocked Controller: real child stdio,
    // process lifetime, loopback audio, UI routing and thread identity are tested.
    const HELPER: &str = r#"#!/usr/bin/env python3
import json, os, sys, time
scenario = json.load(open('scenario.json'))
def emit(value):
    print(json.dumps(value), flush=True)
def note(value):
    with open('rpc.jsonl','a') as f: f.write(json.dumps(value)+'\n')
def call(id, action='composer.write', thread='voice-thread', text='draft'):
    emit({'id':id,'method':'item/tool/call','params':{'threadId':thread,'tool':'vorkspace','callId':'effect-1','arguments':{'action':action,'input':{'text':text}}}})
for line in sys.stdin:
    value=json.loads(line); note(value)
    method=value.get('method'); id=value.get('id'); params=value.get('params',{})
    if method=='initialize': emit({'id':id,'result':{}})
    elif method in ['thread/start','thread/resume']:
        if scenario.get('hold_start'):
            open('start.accepted','w').close()
            while not os.path.exists('release.start'): time.sleep(.005)
        if scenario.get('fail_resume'): emit({'id':id,'error':{'message':'unavailable'}})
        else: emit({'id':id,'result':{'thread':{'id':'different' if scenario.get('changed_id') else 'voice-thread'}}})
    elif method=='thread/turns/list':
        emit({'id':id,'result':{'data':[{'items':[{'type':'agentMessage','text':'previous reply'}]}]}})
    elif method=='thread/realtime/start':
        assert params['flushTranscriptTailOnSessionEnd'] == True
        assert params['includeStartupContext'] == False
        assert os.path.exists('identity.saved')
        emit({'id':id,'result':{}})
        emit({'method':'thread/realtime/sdp','params':{'threadId':'voice-thread','sdp':'answer-sdp'}})
        if scenario.get('tools'): call(100)
        else: emit({'method':'thread/realtime/transcript/done','params':{'threadId':'voice-thread','role':'assistant','text':'ready-for-end'}})
    elif id==100 and method is None: call(101)
    elif id==101 and method is None: call(102, text='changed')
    elif id==102 and method is None: call(103, thread='another-thread')
    elif id==103 and method is None: call(104, action='shell.exec')
    elif id==104 and method is None:
        emit({'method':'thread/realtime/transcript/done','params':{'threadId':'voice-thread','role':'assistant','text':'ready-for-end'}})
    elif method=='thread/realtime/stop':
        call(200, text='late request')
        emit({'id':id,'result':{}})
    elif id==200 and method is None:
        emit({'method':'thread/realtime/closed','params':{'threadId':'voice-thread','reason':'requested'}})
"#;
    struct Fixture {
        dir: tempfile::TempDir,
        host: Host,
        rx: async_channel::Receiver<Value>,
        resumed: bool,
        saves: usize,
        effects: usize,
        ready: bool,
    }
    impl Fixture {
        fn new(scenario: Value, resumed: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let binary = dir.path().join("helper");
            fs::write(&binary, HELPER).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(dir.path().join("scenario.json"), scenario.to_string()).unwrap();
            let (host, rx) = Host::launch(
                dir.path().to_str().unwrap(),
                &dir.path().join("vox.lock"),
                &binary,
            )
            .unwrap();
            Self {
                dir,
                host,
                rx,
                resumed,
                saves: 0,
                effects: 0,
                ready: false,
            }
        }
        fn next(&self) -> Value {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match self.rx.try_recv() {
                    Ok(v) => return v,
                    Err(async_channel::TryRecvError::Closed) => {
                        panic!("controller closed without expected frame")
                    }
                    Err(_) => {}
                }
                assert!(
                    Instant::now() < deadline,
                    "UI timed out; helper traffic: {}",
                    fs::read_to_string(self.dir.path().join("rpc.jsonl")).unwrap_or_default()
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
        fn answer(&mut self, frame: &Value) {
            if frame.get("id").is_none() {
                return;
            }
            let result = match frame["action"].as_str().unwrap() {
                "vox.config" => {
                    json!({"cwd":self.dir.path(),"silent":false,"microphone":{"id":"system"}})
                }
                "workspace.current" => json!({"bots":[]}),
                "memory.read" => json!({"text":""}),
                "vox.conversation.read" => {
                    json!({"threadId":if self.resumed{Some("voice-thread")}else{None},"generation":0})
                }
                "vox.conversation.save" => {
                    assert!(!self.ready, "identity must be durable before audio starts");
                    assert_eq!(frame["input"]["threadId"], "voice-thread");
                    self.saves += 1;
                    fs::write(self.dir.path().join("identity.saved"), "saved").unwrap();
                    Value::Null
                }
                "composer.write" => {
                    self.effects += 1;
                    json!({"sent":false})
                }
                action => panic!("unexpected workspace action: {action}"),
            };
            self.host
                .send(json!({"id":frame["id"],"ok":true,"result":result}));
        }
        fn audio(&mut self, frame: &Value) {
            let url = frame["ready"].as_str().unwrap();
            assert_eq!(self.saves, 1);
            self.ready = true;
            let client = ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(2))
                .build();
            let bad = client
                .get(&format!("{url}config"))
                .set("Origin", "https://untrusted.example")
                .call()
                .unwrap_err();
            assert!(matches!(bad, ureq::Error::Status(403, _)));
            let config: Value = serde_json::from_str(
                &client
                    .get(&format!("{url}config"))
                    .call()
                    .unwrap()
                    .into_string()
                    .unwrap(),
            )
            .unwrap();
            assert!(config.get("token").is_none());
            client
                .post(&format!("{url}event"))
                .send_string(&json!({"type":"offer","sdp":"offer-sdp"}).to_string())
                .unwrap();
        }
        fn until_live(&mut self) {
            loop {
                let frame = self.next();
                assert_ne!(frame["input"]["state"], "idle", "{frame}");
                if frame.get("ready").is_some() {
                    self.audio(&frame);
                }
                if frame["input"]["display"]["lines"][0]["text"] == "ready-for-end" {
                    break;
                }
                self.answer(&frame);
            }
        }
        fn until_idle(&mut self) -> Value {
            loop {
                let frame = self.next();
                if frame["input"]["state"] == "idle" {
                    return frame["input"].clone();
                }
                self.answer(&frame);
            }
        }
        fn finish(&mut self) {
            self.host.worker.take().unwrap().join().unwrap();
        }
        fn traffic(&self) -> Vec<Value> {
            fs::read_to_string(self.dir.path().join("rpc.jsonl"))
                .unwrap()
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect()
        }
    }
    #[test]
    fn native_call_saves_identity_routes_once_and_flushes_on_end() {
        let mut f = Fixture::new(json!({"tools":true}), false);
        f.until_live();
        assert_eq!(f.effects, 1);
        f.host.send(json!({"control":"stop"}));
        assert!(f.until_idle()["error"].is_null());
        f.finish();
        let traffic = f.traffic();
        let results: Vec<_> = traffic
            .iter()
            .filter(|v| v["id"].as_u64().is_some_and(|id| (100..=104).contains(&id)))
            .collect();
        assert_eq!(results.len(), 5);
        assert_eq!(
            traffic.iter().find(|v| v["id"] == 200).unwrap()["result"]["success"],
            false
        );
        assert_eq!(results[0]["result"], results[1]["result"]);
        assert_eq!(results[0]["result"]["success"], true);
        for result in &results[2..] {
            assert_eq!(result["result"]["success"], false);
        }
        assert!(traffic
            .iter()
            .any(|v| v["method"] == "thread/realtime/stop"));
        let (next, _) = Host::launch(
            f.dir.path().to_str().unwrap(),
            &f.dir.path().join("vox.lock"),
            &f.dir.path().join("helper"),
        )
        .unwrap();
        drop(next);
    }
    #[test]
    fn resumed_call_keeps_identity_and_loads_only_its_recent_history() {
        let mut f = Fixture::new(json!({}), true);
        f.until_live();
        f.host.send(json!({"control":"stop"}));
        assert!(f.until_idle()["error"].is_null());
        f.finish();
        let traffic = f.traffic();
        assert!(!traffic.iter().any(|v| v["method"] == "thread/start"));
        let read = traffic
            .iter()
            .find(|v| v["method"] == "thread/turns/list")
            .unwrap();
        assert_eq!(read["params"]["threadId"], "voice-thread");
        assert_eq!(read["params"]["limit"], 4);
        let start = traffic
            .iter()
            .find(|v| v["method"] == "thread/realtime/start")
            .unwrap();
        assert!(start["params"]["initialItems"][0]["text"]
            .as_str()
            .unwrap()
            .contains("previous reply"));
    }
    #[test]
    fn resume_failure_or_changed_identity_never_starts_fresh_or_opens_audio() {
        for scenario in [json!({"fail_resume":true}), json!({"changed_id":true})] {
            let mut f = Fixture::new(scenario, true);
            let idle = f.until_idle();
            assert!(idle["error"].is_string());
            f.finish();
            assert_eq!(f.saves, 0);
            assert!(!f.ready);
            assert!(!f.traffic().iter().any(|v| matches!(
                v["method"].as_str(),
                Some("thread/start" | "thread/realtime/start")
            )));
        }
    }
    #[test]
    fn end_during_thread_start_saves_accepted_identity_without_opening_audio() {
        let mut f = Fixture::new(json!({"hold_start":true}), false);
        // Complete only setup UI requests. The helper then waits for our explicit release.
        for _ in 0..4 {
            let frame = f.next();
            f.answer(&frame);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !f.dir.path().join("start.accepted").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        f.host.send(json!({"control":"stop"}));
        fs::write(f.dir.path().join("release.start"), "go").unwrap();
        assert!(f.until_idle()["error"].is_null());
        f.finish();
        assert_eq!(f.saves, 1);
        assert!(!f.ready);
        assert!(!f
            .traffic()
            .iter()
            .any(|v| v["method"] == "thread/realtime/start"));
    }
    #[test]
    fn audio_failure_flushes_before_helper_exit() {
        let mut f = Fixture::new(json!({}), false);
        f.until_live();
        f.host
            .sender
            .send(Event::Audio(
                json!({"type":"error","message":"microphone disconnected"}),
            ))
            .unwrap();
        assert_eq!(f.until_idle()["error"], "microphone disconnected");
        f.finish();
        assert!(f
            .traffic()
            .iter()
            .any(|v| v["method"] == "thread/realtime/stop"));
    }
    #[test]
    fn window_disposal_flushes_an_admitted_call_without_ui_callbacks() {
        let mut f = Fixture::new(json!({}), false);
        f.until_live();
        let worker = f.host.worker.take().unwrap();
        f.rx.close();
        drop(f.host);
        worker.join().unwrap();
        let traffic = fs::read_to_string(f.dir.path().join("rpc.jsonl")).unwrap();
        let frames: Vec<Value> = traffic
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(frames.iter().any(|v| v["method"] == "thread/realtime/stop"));
        assert_eq!(
            frames.iter().find(|v| v["id"] == 200).unwrap()["result"]["success"],
            false
        );
    }
    #[test]
    fn finished_owner_unlocks_even_when_an_inherited_descriptor_is_still_open() {
        use std::os::fd::AsRawFd;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vox.lock");
        let file = fs::File::create(&path).unwrap();
        assert_eq!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let owner = CallLock(file);
        // dup has the same open-file description as a descriptor copied by fork.
        let inherited = owner.0.try_clone().unwrap();
        drop(owner);
        let next = fs::File::open(&path).unwrap();
        assert_eq!(
            unsafe { libc::flock(next.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        drop(inherited);
    }
    #[test]
    fn active_owner_excludes_other_windows_and_rejects_symlink_locks() {
        let mut f = Fixture::new(json!({}), false);
        assert!(Host::launch(
            f.dir.path().to_str().unwrap(),
            &f.dir.path().join("vox.lock"),
            &f.dir.path().join("helper")
        )
        .is_err());
        let alias = f.dir.path().join("alias.lock");
        std::os::unix::fs::symlink(f.dir.path().join("vox.lock"), &alias).unwrap();
        assert!(Host::launch(
            f.dir.path().to_str().unwrap(),
            &alias,
            &f.dir.path().join("helper")
        )
        .is_err());
        f.host.send(json!({"control":"stop"}));
        f.until_idle();
        f.finish();
    }
}
