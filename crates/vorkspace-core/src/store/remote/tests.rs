use super::*;
use std::path::Path;

fn project(store: &Store, environment: &str) -> RemoteProject {
    store
        .remember_remote_project(environment, &json!({"id":"project","cwd":"/host/repo"}))
        .unwrap()
}
fn snapshot(id: &str) -> Value {
    json!({"projectId":"project","revision":4,"status":"idle","updatedAt":20,
            "session":{"id":id,"harness":"codex","title":"Host conversation","model":"codex:live-model",
                "runtimeMode":"auto","modelSettings":{"reasoningEffort":"high"},"cwd":"/host/repo/worktree",
                "blocks":[{"id":"prompt","role":"user","text":"hello"}]}})
}

#[test]
fn journal_consumes_the_draft_across_first_binding_and_preserves_new_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let project = project(&store, "env");
    let summary = store.create("codex", &project.key).unwrap();
    let id = summary.id;
    let mut input = json!({"cwd":summary.cwd,"text":"Send once","selectionStart":0,"selectionEnd":0,"attachments":[],"mcpTags":[]});
    let revision = store.save_composer_draft(id, 0, &input).unwrap();
    let origin = store.composer_origin(id, revision).unwrap();
    let before = store.remote_session(id).unwrap().unwrap();
    let mut next = before.clone();
    next.outbox = vec![json!({"type":"send","commandId":"send-once","text":"Send once"})];
    next.composer_origin = Some(origin.clone());
    let pending = store.commit_remote_journal(&before, next).unwrap();
    assert!(store.composer_draft(id).unwrap().unwrap().deleted);
    let mut bound = pending.clone();
    bound.session_id = Some("host-assigned".into());
    bound.outbox.clear();
    store.commit_remote_journal(&pending, bound).unwrap();
    drop(store);

    let store = Store::open_vorkspace(&path).unwrap();
    let saved = store.composer_draft(id).unwrap().unwrap();
    assert_eq!(saved.revision, revision);
    assert!(saved.deleted);
    assert!(store.composer_origin(id, revision).is_err());
    input["text"] = json!("Edit after the journal commit");
    let latest = store.save_composer_draft(id, revision, &input).unwrap();
    assert_ne!(store.composer_origin(id, latest).unwrap(), origin);
    assert!(!store.composer_draft(id).unwrap().unwrap().deleted);
}

#[test]
fn a_remote_binding_read_uses_the_current_sidecar_and_isolates_invalid_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let reader = Store::open_vorkspace(&path).unwrap();
    let writer = Store::open_vorkspace(&path).unwrap();
    let local = writer.create("codex", "/local").unwrap();
    let project = project(&writer, "env");
    let remote = writer
        .register_remote_summary(&project, &snapshot("selected"))
        .unwrap();
    let mut raw: Value = serde_json::from_str(&writer.setting(SESSIONS).unwrap().unwrap()).unwrap();
    raw["-1"] = json!({"incomplete": true});
    writer.save_setting(SESSIONS, &raw.to_string()).unwrap();
    assert_eq!(
        reader.remote_session(remote.id).unwrap().unwrap().summary,
        remote
    );
    assert!(reader.remote_session(local.id).unwrap().is_none());
    assert!(reader.remote_session(-1).is_err());

    raw[remote.id.to_string()]["summary"]["title"] = json!("Changed by peer");
    writer.save_setting(SESSIONS, &raw.to_string()).unwrap();
    assert_eq!(
        reader
            .remote_session(remote.id)
            .unwrap()
            .unwrap()
            .summary
            .title,
        "Changed by peer"
    );
    raw.as_object_mut().unwrap().remove(&remote.id.to_string());
    writer.save_setting(SESSIONS, &raw.to_string()).unwrap();
    assert!(reader.remote_session(remote.id).unwrap().is_none());
}

#[test]
fn remote_metadata_and_original_outbox_survive_restart_without_local_sessions_or_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.db");
    let store = Store::open(&path).unwrap();
    let project = project(&store, "env");
    let summary = store.create("codex", &project.key).unwrap();
    assert!(summary.id < 0);
    let mut binding = store.remote_session(summary.id).unwrap().unwrap();
    binding.outbox = vec![
        json!({"type":"create","commandId":"original-create"}),
        json!({"type":"send","commandId":"original-send","text":"keep me"}),
    ];
    binding.uploads = vec![RemoteUpload {
        command_id: "original-send".into(),
        id: uuid::Uuid::new_v4().to_string(),
        path: dir.path().join("photo.png"),
        size: 3,
    }];
    store.save_remote_session(&binding).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let restored = store.remote_session(summary.id).unwrap().unwrap();
    assert_eq!(restored.outbox, binding.outbox);
    assert_eq!(restored.uploads[0].id, binding.uploads[0].id);
    assert_eq!(store.sessions().unwrap()[0].id, summary.id);
    for table in ["native_sessions", "native_rows"] {
        let count: i64 = store
            .conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[test]
fn host_refresh_preserves_selected_handle_pending_commands_and_unsent_settings() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = project(&store, "env");
    let summary = store.create("codex", &project.key).unwrap();
    let mut binding = store.remote_session(summary.id).unwrap().unwrap();
    binding.session_id = Some("host-session".into());
    binding
        .outbox
        .push(json!({"type":"send","commandId":"pending","text":"unsent"}));
    store.save_remote_session(&binding).unwrap();
    let settings = Settings {
        model: Some("codex:unsent-choice".into()),
        revision: 7,
        ..Default::default()
    };
    store.save_agent_settings(summary.id, &settings).unwrap();
    let refreshed = store
        .register_remote_summary(&project, &snapshot("host-session"))
        .unwrap();
    assert_eq!(refreshed.id, summary.id);
    assert_eq!(
        store.remote_session(summary.id).unwrap().unwrap().outbox,
        binding.outbox
    );
    assert_eq!(
        store.agent_settings(summary.id).unwrap().unwrap().model,
        settings.model
    );
    assert_eq!(store.sessions().unwrap().len(), 1);
}

#[test]
fn independent_stores_keep_remote_commands_and_host_metadata_at_their_own_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let observing = Store::open_vorkspace(&path).unwrap();
    let sending = Store::open_vorkspace(&path).unwrap();
    let project = project(&observing, "env");
    let id = observing
        .register_remote_summary(&project, &snapshot("session"))
        .unwrap()
        .id;
    let adjacent = observing
        .register_remote_summary(&project, &snapshot("adjacent"))
        .unwrap();
    let admitted = observing.remote_session(id).unwrap().unwrap();
    let mut pending = admitted.clone();
    pending
        .outbox
        .push(json!({"type":"send","commandId":"kept","text":"unsent"}));
    pending.uploads.push(RemoteUpload {
        command_id: "kept".into(),
        id: "attachment".into(),
        path: dir.path().join("photo.png"),
        size: 3,
    });
    let pending = sending.commit_remote_journal(&admitted, pending).unwrap();

    let mut running = snapshot("session");
    running["revision"] = json!(6);
    running["status"] = json!("running");
    running["pinned"] = json!(true);
    running["session"]["title"] = json!("Renamed on Host");
    observing
        .observe_remote_activity(&admitted, &running, true)
        .unwrap();
    observing
        .observe_remote_activity(&admitted, &snapshot("session"), false)
        .unwrap();
    assert!(observing
        .commit_remote_journal(&admitted, admitted.clone())
        .is_err());

    let mut accepted = pending.clone();
    accepted.outbox.clear();
    accepted.uploads.clear();
    let accepted = sending.commit_remote_journal(&pending, accepted).unwrap();
    assert_eq!(accepted.host_revision, Some(6));
    assert!(accepted.running && accepted.needs_input && accepted.summary.pinned);
    assert_eq!(accepted.summary.title, "Renamed on Host");
    assert!(accepted.outbox.is_empty() && accepted.uploads.is_empty());
    assert_eq!(
        observing
            .remote_session(adjacent.id)
            .unwrap()
            .unwrap()
            .summary,
        adjacent
    );

    sending.forget_remote_binding(&accepted).unwrap();
    assert!(observing
        .observe_remote_activity(&admitted, &running, true)
        .is_err());
    assert!(observing
        .observe_bound_remote_summary(&admitted, &running)
        .is_err());
    assert!(observing.remote_session(id).unwrap().is_none());
}

#[test]
fn a_remote_metadata_receipt_cannot_replace_a_newer_host_summary_or_local_journal() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = project(&store, "env");
    let summary = store
        .register_remote_summary(&project, &snapshot("session"))
        .unwrap();
    let admitted = store.remote_session(summary.id).unwrap().unwrap();
    let mut pending = admitted.clone();
    pending
        .outbox
        .push(json!({"type":"send","commandId":"pending"}));
    let pending = store.commit_remote_journal(&admitted, pending).unwrap();
    let older = json!({"id":"session","projectId":"project","revision":5,"title":"Earlier edit"});
    let newer = json!({"id":"session","projectId":"project","revision":6,"title":"Later edit"});
    store
        .observe_bound_remote_summary(&admitted, &newer)
        .unwrap();
    let current = store
        .observe_bound_remote_summary(&admitted, &older)
        .unwrap();
    assert_eq!(current.summary.title, "Later edit");
    assert_eq!(current.outbox, pending.outbox);
    assert!(store
        .observe_bound_remote_summary(&admitted, &json!({"id":"session","projectId":"other"}))
        .is_err());
}

#[test]
fn imported_host_handles_are_scoped_to_environment_and_use_remote_worktree_paths() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let one = project(&store, "one");
    let two = project(&store, "two");
    let first = store
        .register_remote_summary(&one, &snapshot("same-host-id"))
        .unwrap();
    let second = store
        .register_remote_summary(&two, &snapshot("same-host-id"))
        .unwrap();
    assert_ne!(first.id, second.id);
    let settings = store.agent_settings(first.id).unwrap().unwrap();
    assert_eq!(
        settings.worktree_cwd.as_deref(),
        Some("remote://one/host/repo/worktree")
    );
    assert_eq!(settings.access, crate::agent::AccessMode::Auto);
    assert_eq!(settings.model_settings["reasoningEffort"], "high");
}

#[test]
fn resolved_approval_strings_do_not_keep_needs_input_set() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = project(&store, "env");
    for (revision, decision) in [
        Value::Null,
        json!("allow"),
        json!("deny"),
        json!("cancelled"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut value = snapshot("session");
        // A changed approval is published as a new host revision.
        value["revision"] = json!(revision + 1);
        value["session"]["blocks"] =
            json!([{"id":"tool","role":"tool","approval":{"requestId":3,"decided":decision}}]);
        let summary = store.register_remote_summary(&project, &value).unwrap();
        assert_eq!(
            store
                .remote_session(summary.id)
                .unwrap()
                .unwrap()
                .needs_input,
            decision.is_null()
        );
    }
}

#[test]
fn a_listing_cannot_prune_a_binding_created_or_updated_while_it_was_pending() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = project(&store, "env");
    let gone = store
        .register_remote_summary(&project, &snapshot("gone"))
        .unwrap();
    let current = store
        .register_remote_summary(&project, &snapshot("current"))
        .unwrap();
    let baseline = store.remote_listing_baseline(&project).unwrap();
    let new = store
        .register_remote_summary(&project, &snapshot("new"))
        .unwrap();
    let mut newer = snapshot("current");
    newer["revision"] = json!(100);
    store.register_remote_summary(&project, &newer).unwrap();
    let listing = store
        .apply_remote_listing(&project, &[], &baseline)
        .unwrap();
    assert_eq!(listing.removed, [gone.id]);
    assert!(store.remote_session(current.id).unwrap().is_some());
    assert!(store.remote_session(new.id).unwrap().is_some());
}

#[test]
fn pruning_external_deletions_keeps_pending_and_unbound_sessions() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = project(&store, "env");
    let blank = store.create("codex", &project.key).unwrap();
    let gone = store
        .register_remote_summary(&project, &snapshot("gone"))
        .unwrap();
    let pending = store
        .register_remote_summary(&project, &snapshot("pending"))
        .unwrap();
    let mut binding = store.remote_session(pending.id).unwrap().unwrap();
    binding.outbox.push(json!({"commandId":"keep-original"}));
    store.save_remote_session(&binding).unwrap();
    let present = store
        .register_remote_summary(&project, &snapshot("present"))
        .unwrap();
    let rejected = store
        .register_remote_summary(&project, &snapshot("rejected"))
        .unwrap();
    let mut rejected_binding = store.remote_session(rejected.id).unwrap().unwrap();
    rejected_binding.rejected = Some(json!({"commands":[{"text":"Keep this original"}]}));
    store.save_remote_session(&rejected_binding).unwrap();
    let other = self::project(&store, "other-env");
    let elsewhere = store
        .register_remote_summary(&other, &snapshot("gone"))
        .unwrap();
    let baseline = store.remote_listing_baseline(&project).unwrap();
    let listing = store
        .apply_remote_listing(
            &project,
            &[json!({"id":"present","revision":3,"title":"Stale","status":"running"})],
            &baseline,
        )
        .unwrap();
    assert_eq!(listing.removed, [gone.id]);
    assert_eq!(listing.entries[0].summary, present);
    assert!(!listing.entries[0].running);
    assert!(store.remote_session(gone.id).unwrap().is_none());
    for id in [blank.id, pending.id, present.id, rejected.id, elsewhere.id] {
        assert!(store.remote_session(id).unwrap().is_some());
    }
}

#[test]
fn listing_and_seed_preferences_publish_together_after_every_row_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vorkspace.db");
    let store = Store::open_vorkspace(&path).unwrap();
    let project = project(&store, "env");
    let broken = store
        .register_remote_summary(&project, &snapshot("broken"))
        .unwrap();
    let baseline = store.remote_listing_baseline(&project).unwrap();
    let broken_key = agent_key(broken.id);
    store.save_setting(&broken_key, "{").unwrap();
    let sidecar = path.with_file_name("vorkspace.db.gpui.json");
    let before = std::fs::read(&sidecar).unwrap();
    let fresh = json!({"id":"fresh","revision":1,"title":"New","status":"running","needsInput":true,"draft":true});
    let changed = json!({"id":"broken","revision":5,"title":"Changed"});

    let err = store
        .apply_remote_listing(&project, &[fresh.clone(), changed.clone()], &baseline)
        .unwrap_err();
    assert!(err.starts_with("Cannot read remote settings"));
    assert_eq!(std::fs::read(&sidecar).unwrap(), before);
    assert_eq!(
        store.sessions().unwrap().as_slice(),
        std::slice::from_ref(&broken)
    );

    store.save_setting(&broken_key, "{}").unwrap();
    let listing = store
        .apply_remote_listing(&project, &[fresh, changed], &baseline)
        .unwrap();
    assert!(listing.removed.is_empty());
    let id = listing.entries[0].summary.id;
    assert!(listing.entries[0].running && listing.entries[0].needs_input);
    assert!(listing.entries[0].draft);
    drop(store);
    let reopened = Store::open_vorkspace(&path).unwrap();
    assert!(reopened.agent_preferences(id).unwrap().is_some());
    assert_eq!(
        reopened
            .remote_session(broken.id)
            .unwrap()
            .unwrap()
            .summary
            .title,
        "Changed"
    );
}

#[test]
fn windows_project_keys_normalize_trailing_separators_and_find_legacy_unc_keys() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = store
        .remember_remote_project("env", &json!({"id":"project","cwd":r"C:\repo\"}))
        .unwrap();
    assert_eq!(project.key, "remote://env/C:/repo");
    let legacy = RemoteProject {
        key: "remote://env/server/share/repo".into(),
        environment_id: "env".into(),
        project_id: "unc".into(),
        cwd: r"\\server\share\repo".into(),
    };
    store
        .save_setting(
            PROJECTS,
            &serde_json::to_string(&BTreeMap::from([(legacy.key.clone(), legacy)])).unwrap(),
        )
        .unwrap();
    assert!(store
        .create("codex", "remote://env//server/share/repo")
        .is_ok());
}
#[test]
fn remote_projection_matches_full_history_through_reorders_and_truncation() {
    use crate::remote::protocol::SessionObservation;

    let mut observation = SessionObservation::default();
    let mut projection = RemoteHistory::default();
    let mut rendered = Vec::new();
    for update in [
        json!({"kind":"snapshot","value":{"revision":1,"session":{"id":"s","blocks":[
            {"id":"u","role":"user","text":"Keep the attachment","attachments":[{"id":"pic","size":3,"path":"/host/image.png"}]},
            {"id":"p","role":"plan","text":"Review the change"},
            {"id":"d","role":"user","text":"Draft","draft":true}
        ]}}}),
        json!({"kind":"delta","base":1,"value":{"revision":2,"session":{"id":"s"}},
            "blockIds":["u","p","d","a"],"blocks":[{"id":"a","role":"assistant","text":"Start"}]}),
        json!({"kind":"delta","base":2,"value":{"revision":3,"session":{"id":"s"}},
            "blockIds":["u","a","p"],"blocks":[{"id":"a","role":"assistant","text":"Finished"}]}),
        json!({"kind":"delta","base":3,"value":{"revision":4,"session":{"id":"s"}},
            "blockIds":["u"],"blocks":[]}),
        json!({"kind":"delta","base":4,"value":{"revision":5,"session":{"id":"s"}},
            "blockIds":[],"blocks":[]}),
    ] {
        let first = observation.apply(update).unwrap().unwrap();
        let snapshot = observation.snapshot().unwrap();
        let (start, rows) = projection.update(snapshot, first, "env", "s").unwrap();
        rendered.truncate(start);
        rendered.extend(rows);
        let (full, _) = Store::project_remote_history(snapshot, "env", "s").unwrap();
        assert_eq!(rendered, full.rows);
        assert_eq!(projection.rows(), rendered);
    }
}
