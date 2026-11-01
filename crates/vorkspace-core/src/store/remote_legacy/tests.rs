
use super::*;
use crate::{agent::Settings, desktop_preferences::DesktopPreferences};
use std::{path::Path, sync::Arc};

const PROJECT: &str = "remote://legacy/home/repo";

fn fixture(path: &Path) -> (i64, String, i64, String) {
    let store = Store::open_vorkspace(path).unwrap();
    let bound = store.local_create("codex", PROJECT).unwrap();
    let blank = store.local_create("claude", PROJECT).unwrap();
    let bound_original = store.persisted_session_id(bound.id).unwrap();
    let blank_original = store.persisted_session_id(blank.id).unwrap();
    let settings = Settings {
        model: Some("gpt-saved".into()),
        ..Default::default()
    };
    store.save_agent_settings(bound.id, &settings).unwrap();
    (bound.id, bound_original, blank.id, blank_original)
}

fn preferences(bound: &str, blank: &str) -> DesktopPreferences {
    let command_key = format!(
        "vorkspace.remote-command.v1:{}:pending",
        json!([PROJECT, "legacy"])
    );
    DesktopPreferences::from_values(&[
        (
            "vorkspace.remote-projects.v2",
            json!({PROJECT:{"key":PROJECT,"environmentId":"legacy","projectId":"host-project","cwd":"/home/repo"}}),
        ),
        ("vorkspace.remote-tabs.v2", json!({bound:"host-session"})),
        (
            "vorkspace.remote-pending-worktrees.v1",
            json!({bound:"/home/repo-worktrees/old",blank:"/home/repo-worktrees/pending"}),
        ),
        (
            &format!("vorkspace.remote-history.v2:{PROJECT}"),
            json!([{"id":"host-session","title":"Host title","harness":"codex","cwd":"/home/repo-worktrees/current","updatedAt":123,"pinned":true}]),
        ),
        (
            &command_key,
            json!({"shellId":blank,"command":{"type":"create","commandId":"original-create","projectId":"host-project","harness":"claude","model":"claude:opus"},"followup":{"type":"send","commandId":"original-send","sessionId":"","text":"Keep this exact pending text"}}),
        ),
    ])
}

fn bare_preferences(bound: &str, blank: &str, ambiguous: bool) -> DesktopPreferences {
    let source = preferences(bound, blank);
    let entries: Vec<(String, Value)> = source
        .entries()
        .map(|(key, raw)| {
            let mut value: Value = serde_json::from_str(raw).unwrap();
            if key.starts_with("vorkspace.remote-command.v1:") {
                value = value["command"].clone();
            }
            if ambiguous && key == "vorkspace.remote-tabs.v2" {
                value = json!({});
            }
            (key.into(), value)
        })
        .collect();
    DesktopPreferences::from_values(
        &entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>(),
    )
}

#[test]
fn bare_create_uses_the_unique_unbound_tab_and_keeps_its_claim_after_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.db");
    let (_, original, blank, blank_original) = fixture(&path);
    let prefs = Arc::new(bare_preferences(&original, &blank_original, false));
    let store = Store::open_vorkspace_with_preferences_for_test(&path, prefs).unwrap();
    let mut remote = store.remote_session(blank).unwrap().unwrap();
    assert_eq!(remote.outbox.len(), 1);
    assert_eq!(remote.outbox[0]["commandId"], "original-create");
    store.save_remote_session(&remote).unwrap();
    remote.session_id = Some("accepted-host-session".into());
    remote.outbox.clear();
    store.save_remote_session(&remote).unwrap();
    let later = store.local_create("codex", PROJECT).unwrap();
    let later = store.remote_session(later.id).unwrap().unwrap();
    assert!(later.outbox.is_empty());
    assert!(later.rejected.is_none());
}

#[test]
fn ambiguous_bare_create_is_visible_recovery_state_without_a_replayable_outbox() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.db");
    let (bound, original, blank, blank_original) = fixture(&path);
    let before = std::fs::read(&path).unwrap();
    let prefs = Arc::new(bare_preferences(&original, &blank_original, true));
    let store = Store::read_vorkspace_with_preferences_for_test(&path, prefs).unwrap();
    for id in [bound, blank] {
        let remote = store.remote_session(id).unwrap().unwrap();
        assert!(remote.outbox.is_empty());
        let recovery = remote.rejected.unwrap();
        assert_eq!(recovery["legacyRecovery"], true);
        assert_eq!(recovery["commands"][0]["commandId"], "original-create");
        assert!(recovery["error"]
            .as_str()
            .unwrap()
            .contains("cannot be linked"));
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn multiple_bare_creates_cannot_be_replayed_into_one_blank_tab() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.db");
    let (_, original, blank, blank_original) = fixture(&path);
    let source = bare_preferences(&original, &blank_original, false);
    let mut entries: Vec<(String, Value)> = source
        .entries()
        .map(|(key, raw)| (key.into(), serde_json::from_str(raw).unwrap()))
        .collect();
    entries.push((format!("vorkspace.remote-command.v1:{}:second",json!([PROJECT,"legacy"])),json!({"type":"create","commandId":"second-create","projectId":"host-project","harness":"claude","model":"claude:opus"})));
    let prefs = DesktopPreferences::from_values(
        &entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>(),
    );
    let store = Store::read_vorkspace_with_preferences_for_test(&path, Arc::new(prefs)).unwrap();
    let remote = store.remote_session(blank).unwrap().unwrap();
    assert!(remote.outbox.is_empty());
    let recovery = remote.rejected.unwrap();
    assert_eq!(recovery["legacyRecovery"], true);
    assert_eq!(recovery["commands"].as_array().unwrap().len(), 2);
}

#[test]
fn legacy_remote_tabs_restore_positive_handles_settings_and_outbox_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.db");
    let (bound, original, blank, blank_original) = fixture(&path);
    let before = std::fs::read(&path).unwrap();
    let prefs = Arc::new(preferences(&original, &blank_original));
    let store = Store::read_vorkspace_with_preferences_for_test(&path, prefs.clone()).unwrap();
    let rows = store.sessions().unwrap();
    assert_eq!(rows.len(), 2);
    let remote = store.remote_session(bound).unwrap().unwrap();
    assert!(remote.summary.id > 0);
    assert_eq!(remote.summary.id, bound);
    assert_eq!(remote.session_id.as_deref(), Some("host-session"));
    assert_eq!(remote.summary.title, "Host title");
    assert_eq!(store.persisted_session_id(bound).unwrap(), original);
    assert_eq!(
        store
            .agent_settings(bound)
            .unwrap()
            .unwrap()
            .model
            .as_deref(),
        Some("gpt-saved")
    );
    assert_eq!(
        store.legacy_remote_worktree(bound).unwrap().as_deref(),
        Some("remote://legacy/home/repo-worktrees/current")
    );
    assert_eq!(
        store.legacy_remote_worktree(blank).unwrap().as_deref(),
        Some("remote://legacy/home/repo-worktrees/pending")
    );
    let draft = store.remote_session(blank).unwrap().unwrap();
    assert!(draft.session_id.is_none());
    assert_eq!(draft.outbox.len(), 2);
    assert_eq!(draft.outbox[0]["commandId"], "original-create");
    assert_eq!(draft.outbox[1]["commandId"], "original-send");
    assert!(draft.outbox[1]["sessionId"].is_null());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        prefs.json("vorkspace.remote-tabs.v2").unwrap()[original],
        "host-session"
    );
}

#[test]
fn native_host_refresh_reuses_legacy_handle_and_deletion_cannot_resurrect_shell_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.db");
    let (bound, original, _, blank_original) = fixture(&path);
    let prefs = Arc::new(preferences(&original, &blank_original));
    let store = Store::open_vorkspace_with_preferences_for_test(&path, prefs).unwrap();
    let project = store.remote_projects().unwrap()[PROJECT].clone();
    let refreshed=store.register_remote_summary(&project,&json!({"id":"host-session","title":"Fresh host title","harness":"codex","updatedAt":456})).unwrap();
    assert_eq!(refreshed.id, bound);
    assert_eq!(
        store
            .sessions()
            .unwrap()
            .iter()
            .filter(|row| row.id == bound)
            .count(),
        1
    );
    store
        .prune_remote_sessions(&project, &HashSet::new())
        .unwrap();
    assert!(store.remote_session(bound).unwrap().is_none());
    assert!(!store.sessions().unwrap().iter().any(|row| row.id == bound));
    let source_id: String = store
        .conn
        .query_row("SELECT id FROM sessions WHERE id=?1", [&original], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(source_id, original);
}
