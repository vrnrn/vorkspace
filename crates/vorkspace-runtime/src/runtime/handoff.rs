//! App.tsx / handoffTurn.ts's bounded recap from the outgoing bound provider.
use super::*;
use std::time::Instant;
use std::{path::Path, sync::mpsc};
use vorkspace_core::agent::{AccessMode, Decision, Handoff, ProviderSwitch};

pub(super) fn prepare(
    session: i64,
    source: &ProviderSwitch,
    cwd: &str,
    user_request: &str,
    rows: &[Row],
    root: Option<std::path::PathBuf>,
    user_cancel: Arc<AtomicBool>,
) -> Handoff {
    let kind = Kind::from_id(&source.from);
    let cwd = if source.cwd.is_empty() {
        cwd
    } else {
        &source.cwd
    };
    let mut brief = String::new();
    let mut binding = source.provider_id.clone();
    let has_edits = rows.iter().filter(|row| row.role == "tool").any(|row| {
        serde_json::from_str::<vorkspace_core::tool::ToolCall>(&row.text)
            .ok()
            .is_some_and(|tool| {
                matches!(
                    tool.summary().0.as_str(),
                    "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
                )
            })
    });
    if !source.skip_outgoing_recap
        && source.provider_id.is_some()
        && has_edits
        && !user_cancel.load(Ordering::Relaxed)
        && let Some(binary) = provider::resolve(kind)
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let timer_cancel = cancel.clone();
        let cancelled_turn = user_cancel.clone();
        let (settled, settlement) = mpsc::channel();
        let timer = std::thread::spawn(move || {
            let started = Instant::now();
            loop {
                if cancelled_turn.load(Ordering::Relaxed)
                    || started.elapsed() >= Duration::from_secs(45)
                {
                    timer_cancel.store(true, Ordering::Relaxed);
                    break;
                }
                match settlement.recv_timeout(Duration::from_millis(25)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });
        let mut account = source.account.clone();
        if account.id != vorkspace_core::accounts::DEFAULT && account.root.is_none() {
            account.root = root;
        }
        let request = Request {
            kind,
            binary,
            cwd: cwd.into(),
            prompt: vorkspace_core::response::outgoing_handoff_prompt(user_request),
            provider_id: source.provider_id.clone(),
            model: source.model.clone(),
            settings: Settings {
                account,
                model: source.model.clone(),
                model_settings: source.model_settings.clone(),
                effort: source.effort.clone(),
                fast: source.fast,
                service_tier: source.service_tier.clone(),
                thinking: source.thinking,
                access: AccessMode::Supervised,
                ..Default::default()
            },
        };
        let (controls, control_rx) = mpsc::channel();
        let _ = vorkspace_core::process::owned_turn(session, &user_cancel, || {
            provider::run_with_controls(request, cancel, Some(control_rx), |events| {
                for event in events {
                    match event {
                        Event::Text(text) => {
                            // The retained packet is at most 1,800 characters.
                            // Ignore an unhelpful oversized recap without
                            // introducing another provider failure condition.
                            if brief.len() < 64 * 1024 {
                                let mut end = text.len().min(64 * 1024 - brief.len());
                                while !text.is_char_boundary(end) {
                                    end -= 1;
                                }
                                brief.push_str(&text[..end]);
                            }
                        }
                        Event::SessionId(id) => binding = Some(id),
                        Event::Approval(request) => {
                            let _ = controls.send(Control {
                                id: request.id,
                                decision: Decision::Deny,
                            });
                        }
                        Event::Question(request) => {
                            let _ = controls.send(Control {
                                id: request.id,
                                decision: Decision::Deny,
                            });
                        }
                        _ => {}
                    }
                }
                Ok(())
            })
        });
        let _ = settled.send(());
        let _ = timer.join();
    }
    if let Some(id) = &source.provider_id {
        provider::forget_session(kind, id, &source.account.id, Path::new(cwd));
    }
    if let Some(id) = binding.filter(|id| Some(id) != source.provider_id.as_ref()) {
        provider::forget_session(kind, &id, &source.account.id, Path::new(cwd));
    }
    Handoff {
        from: source.from.clone(),
        brief: vorkspace_core::response::choose_handoff_brief(&brief, rows),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_retained_handoff_survives_failed_start_and_is_consumed_only_after_acceptance() {
        for (shared, provider) in [
            (false, "codex"),
            (true, "codex"),
            (false, "omp"),
            (true, "omp"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let open = if shared {
                Store::open_vorkspace
            } else {
                Store::open
            };
            let store = open(&root.join("sessions.db")).unwrap();
            let summary = store.create(provider, root.to_str().unwrap()).unwrap();
            let mut settings = Settings {
                revision: 9,
                text_only: true,
                handoff: Some(Handoff {
                    from: "claude".into(),
                    brief: "Preserve the prepared project recap".into(),
                }),
                ..Default::default()
            };
            store.save_agent_preferences(summary.id, &settings).unwrap();
            let store = Arc::new(Mutex::new(store));
            let (tx, rx) = async_channel::bounded(64);
            let (_control, controls) = mpsc::channel();
            run_turn_using(
                TurnInput {
                    summary: summary.clone(),
                    prompt: "First continuation never reached the provider".into(),
                    settings,
                    composer_origin: None,
                },
                controls,
                store.clone(),
                Arc::new(AtomicBool::new(false)),
                tx,
                |_| None,
            );
            let updates: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert_eq!(
                updates.last().unwrap().settlement().unwrap().outcome,
                TurnOutcome::Failed
            );
            settings = locked(&store)
                .unwrap()
                .agent_preferences(summary.id)
                .unwrap()
                .unwrap();
            settings.text_only = true;
            let retained = settings.handoff.as_ref().unwrap();
            assert!(
                retained
                    .brief
                    .contains("Preserve the prepared project recap")
            );
            assert!(
                retained
                    .brief
                    .contains("First continuation never reached the provider")
            );
            assert_eq!(settings.revision, 9);
            let projected = updates.last().unwrap().handoff.as_ref().unwrap();
            assert_eq!(projected.as_ref(), Some(retained));

            let binary = root.join("mock-provider");
            std::fs::write(
                &binary,
                r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    value=json.loads(line)
    kind=value.get('type')
    if kind:
        data={'sessionId':'handoff-thread'} if kind=='get_state' else {}
        if kind=='prompt':
            with open('accepted-request.json','w') as output:
                json.dump(value,output)
            print(json.dumps({'type':'message_end','message':{'role':'assistant','content':[{'type':'text','text':'Continued from the recap'}]}}),flush=True)
            data={'agentInvoked':False}
        print(json.dumps({'type':'response','id':value.get('id'),'command':kind,'success':True,'data':data}),flush=True)
        if kind=='prompt': break
        continue
    method=value.get('method')
    if method=='initialize':
        print(json.dumps({'id':value['id'],'result':{}}),flush=True)
    elif method=='thread/start':
        print(json.dumps({'id':value['id'],'result':{'thread':{'id':'handoff-thread'}}}),flush=True)
    elif method=='turn/start':
        with open('accepted-request.json','w') as output:
            json.dump(value,output)
        print(json.dumps({'id':value['id'],'result':{'turn':{'id':'accepted'}}}),flush=True)
        print(json.dumps({'method':'item/agentMessage/delta','params':{'itemId':'answer','delta':'Continued from the recap'}}),flush=True)
        print(json.dumps({'method':'turn/completed','params':{'turn':{'status':'completed'}}}),flush=True)
        break
"#,
            )
            .unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
            let (tx, rx) = async_channel::bounded(64);
            let (_control, controls) = mpsc::channel();
            run_turn_using(
                TurnInput {
                    summary: summary.clone(),
                    prompt: "Continue again".into(),
                    settings,
                    composer_origin: None,
                },
                controls,
                store.clone(),
                Arc::new(AtomicBool::new(false)),
                tx,
                |_| Some(binary),
            );
            let updates: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert_eq!(
                updates.last().unwrap().settlement().unwrap().outcome,
                TurnOutcome::Completed
            );
            assert_eq!(updates.last().unwrap().handoff, Some(None));
            let saved = locked(&store)
                .unwrap()
                .agent_preferences(summary.id)
                .unwrap()
                .unwrap();
            assert!(saved.handoff.is_none());
            assert_eq!(saved.revision, 9);
            let request = std::fs::read_to_string(root.join("accepted-request.json")).unwrap();
            assert!(request.contains("Preserve the prepared project recap"));
            assert!(request.contains("First continuation never reached the provider"));
            assert!(request.contains("Continue again"));
        }
    }
}
