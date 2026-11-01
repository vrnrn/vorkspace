use super::*;
use std::path::Path;
fn host(model: &str, settings: Value) -> Value {
    json!({"projectId":"project","revision":3,"status":"idle","updatedAt":1,"session":{
            "id":"host-session","harness":"claude","title":"Host","model":model,"runtimeMode":"auto",
            "modelSettings":settings,"cwd":"/host/repo","blocks":[]}})
}
fn project(store: &Store) -> RemoteProject {
    store
        .remember_remote_project("env", &json!({"id":"project","cwd":"/host/repo"}))
        .unwrap()
}
#[test]
fn imported_common_controls_match_host_instead_of_overwriting_fast_and_effort() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = project(&store);
    let value = host(
        "claude:opus",
        json!({"fast":"true","effort":"max","thinking":"true","context":"1m"}),
    );
    let summary = store.register_remote_summary(&project, &value).unwrap();
    let settings = store.agent_settings(summary.id).unwrap().unwrap();
    assert!(settings.fast);
    assert_eq!(settings.effort.as_deref(), Some("max"));
    assert_eq!(settings.thinking, Some(true));
    let wire = crate::remote::protocol::wire_model_settings("claude", &settings);
    assert_eq!(wire["fast"], "true");
    assert_eq!(wire["effort"], "max");
    assert_eq!(wire["context"], "1m");
}
#[test]
fn external_host_configuration_updates_unmodified_native_settings_and_revision() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = project(&store);
    let first = host("claude:sonnet", json!({}));
    let summary = store.register_remote_summary(&project, &first).unwrap();
    let revision = store.agent_settings(summary.id).unwrap().unwrap().revision;
    let mut next = host("claude:opus", json!({"fast":"true","effort":"high"}));
    next["revision"] = json!(4);
    next["session"]["runtimeMode"] = json!("full-access");
    store.register_remote_summary(&project, &next).unwrap();
    let settings = store.agent_settings(summary.id).unwrap().unwrap();
    assert_eq!(settings.model.as_deref(), Some("claude:opus"));
    assert!(settings.fast);
    assert_eq!(settings.access, crate::agent::AccessMode::FullAccess);
    assert!(settings.revision > revision);
    // Summary listings omit modelSettings and must not erase the baseline.
    let before = store
        .remote_session(summary.id)
        .unwrap()
        .unwrap()
        .configuration;
    store.register_remote_summary(&project,&json!({"id":"host-session","harness":"claude","title":"Listed","model":"claude:opus","runtimeMode":"full-access","updatedAt":10})).unwrap();
    assert_eq!(
        store
            .remote_session(summary.id)
            .unwrap()
            .unwrap()
            .configuration,
        before
    );
    assert!(store.agent_settings(summary.id).unwrap().unwrap().fast);
}
#[test]
fn unsent_override_survives_host_changes_then_external_updates_resume_after_acceptance() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = project(&store);
    let summary = store
        .register_remote_summary(&project, &host("claude:sonnet", json!({})))
        .unwrap();
    let mut local = store.agent_settings(summary.id).unwrap().unwrap();
    local.model = Some("claude:local-choice".into());
    local.revision += 1;
    store.save_agent_settings(summary.id, &local).unwrap();
    let mut external = host("claude:external-choice", json!({"effort":"high"}));
    external["revision"] = json!(4);
    store.register_remote_summary(&project, &external).unwrap();
    assert_eq!(
        store.agent_settings(summary.id).unwrap().unwrap().model,
        local.model
    );
    let accepted = crate::remote::protocol::wire_model_settings("claude", &local);
    let mut accepted = host("claude:local-choice", json!(accepted));
    accepted["revision"] = json!(5);
    store.register_remote_summary(&project, &accepted).unwrap();
    let mut next = host("claude:next-external", json!({"effort":"max"}));
    next["revision"] = json!(6);
    store.register_remote_summary(&project, &next).unwrap();
    assert_eq!(
        store
            .agent_settings(summary.id)
            .unwrap()
            .unwrap()
            .model
            .as_deref(),
        Some("claude:next-external")
    );
}

#[test]
fn older_and_duplicate_host_snapshots_cannot_undo_receipts_or_newer_configuration() {
    for shared in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("remote.db");
        let open = if shared {
            Store::open_vorkspace
        } else {
            Store::open
        };
        let store = open(&path).unwrap();
        let project = project(&store);
        let original = host("claude:sonnet", json!({}));
        let summary = store.register_remote_summary(&project, &original).unwrap();
        let mut settings = store.agent_settings(summary.id).unwrap().unwrap();
        settings.model = Some("claude:chosen".into());
        settings.revision += 1;
        store.save_agent_settings(summary.id, &settings).unwrap();
        let mut journal = store.remote_session(summary.id).unwrap().unwrap();
        journal
            .outbox
            .push(json!({"commandId":"configure-once","type":"configure"}));
        store.save_remote_session(&journal).unwrap();

        let mut accepted = host("claude:chosen", json!({}));
        accepted["revision"] = json!(4);
        accepted["status"] = json!("running");
        accepted["session"]["title"] = json!("New host title");
        store.register_remote_summary(&project, &accepted).unwrap();
        // A receipt finishes using the worker's pre-refresh binding.
        // It clears its own outbox but cannot erase newer host metadata.
        journal.outbox.clear();
        store.save_remote_session(&journal).unwrap();
        let current = store.remote_session(summary.id).unwrap().unwrap();
        assert!(current.outbox.is_empty());
        assert!(current.running);
        assert_eq!(current.summary.title, "New host title");
        assert_eq!(current.host_revision, Some(4));
        assert_eq!(current.configuration_revision, Some(4));

        let mut newest = host("claude:external", json!({"effort":"high"}));
        newest["revision"] = json!(5);
        store.register_remote_summary(&project, &newest).unwrap();
        assert_eq!(
            store
                .agent_settings(summary.id)
                .unwrap()
                .unwrap()
                .model
                .as_deref(),
            Some("claude:external")
        );
        let expected = serde_json::to_value(store.remote_session(summary.id).unwrap()).unwrap();
        let expected_settings =
            serde_json::to_value(store.agent_settings(summary.id).unwrap()).unwrap();
        drop(store);
        let store = open(&path).unwrap();
        let mut duplicate = original.clone();
        duplicate["revision"] = json!(5);
        let mut unversioned = original.clone();
        unversioned.as_object_mut().unwrap().remove("revision");
        for stale in [original, accepted, duplicate, unversioned] {
            store.register_remote_summary(&project, &stale).unwrap();
            assert_eq!(
                serde_json::to_value(store.remote_session(summary.id).unwrap()).unwrap(),
                expected
            );
            assert_eq!(
                serde_json::to_value(store.agent_settings(summary.id).unwrap()).unwrap(),
                expected_settings
            );
        }
    }
}

#[test]
fn same_revision_full_snapshot_completes_a_partial_listing_without_accepting_older_data() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = project(&store);
    let listed = json!({"id":"host-session","revision":3,"harness":"claude",
            "title":"Listed","model":"claude:sonnet","runtimeMode":"auto","status":"idle",
            "providerSessionId":"provider-thread","archived":true,"pinned":true});
    let summary = store.register_remote_summary(&project, &listed).unwrap();
    let partial = store.remote_session(summary.id).unwrap().unwrap();
    assert_eq!(partial.host_revision, Some(3));
    assert_eq!(partial.configuration_revision, None);
    assert!(partial.configuration.is_none());
    let full = host("claude:sonnet", json!({"fast":"true","effort":"max"}));
    store.register_remote_summary(&project, &full).unwrap();
    let settings = store.agent_settings(summary.id).unwrap().unwrap();
    assert!(settings.fast);
    assert_eq!(settings.effort.as_deref(), Some("max"));
    let complete = store.remote_session(summary.id).unwrap().unwrap();
    assert_eq!(complete.configuration_revision, Some(3));
    assert!(complete.summary.archived && complete.summary.pinned);
    assert_eq!(
        complete.summary.provider_id.as_deref(),
        Some("provider-thread")
    );
    assert!(!complete.summary.has_messages);
    store.register_remote_summary(&project, &listed).unwrap();
    assert_eq!(
        serde_json::to_value(store.remote_session(summary.id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&complete).unwrap()
    );

    // A newer partial view must retain fields it does not carry, and an
    // older full view cannot replace it merely because it is more detailed.
    store
        .register_remote_summary(
            &project,
            &json!({"id":"host-session","revision":4,"title":"New listing"}),
        )
        .unwrap();
    let newer = store.remote_session(summary.id).unwrap().unwrap();
    assert_eq!(newer.host_revision, Some(4));
    assert_eq!(newer.configuration_revision, Some(3));
    assert_eq!(newer.configuration, complete.configuration);
    assert!(!newer.summary.has_messages);
    assert!(newer.summary.archived && newer.summary.pinned);
    store.register_remote_summary(&project, &full).unwrap();
    assert_eq!(
        store
            .remote_session(summary.id)
            .unwrap()
            .unwrap()
            .summary
            .title,
        "New listing"
    );
    let mut full = full;
    full["revision"] = json!(4);
    full["session"]["model"] = json!("claude:newer");
    store.register_remote_summary(&project, &full).unwrap();
    assert_eq!(
        store
            .agent_settings(summary.id)
            .unwrap()
            .unwrap()
            .model
            .as_deref(),
        Some("claude:newer")
    );
}

#[test]
fn unversioned_legacy_hosts_remain_unknown_and_can_refresh_configuration() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = project(&store);
    let mut first = host("claude:sonnet", json!({}));
    first.as_object_mut().unwrap().remove("revision");
    let summary = store.register_remote_summary(&project, &first).unwrap();
    let mut next = host("claude:opus", json!({"fast":"true"}));
    next.as_object_mut().unwrap().remove("revision");
    store.register_remote_summary(&project, &next).unwrap();
    let saved = store.remote_session(summary.id).unwrap().unwrap();
    assert_eq!(saved.host_revision, None);
    assert_eq!(saved.configuration_revision, None);
    assert!(store.agent_settings(summary.id).unwrap().unwrap().fast);
    // Previously persisted bindings have neither revision field.
    let mut old = serde_json::to_value(saved).unwrap();
    old.as_object_mut().unwrap().remove("host_revision");
    old.as_object_mut()
        .unwrap()
        .remove("configuration_revision");
    let restored: RemoteSession = serde_json::from_value(old).unwrap();
    assert_eq!(restored.host_revision, None);
    assert_eq!(restored.configuration_revision, None);
}
