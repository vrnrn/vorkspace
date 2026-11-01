
use super::*;

fn prefs(entries: &[(&str, Value)]) -> DesktopPreferences {
    DesktopPreferences {
        available: true,
        values: entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    }
}
fn origin(scheme: &str, host: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in [scheme, host] {
        bytes.extend((value.len() as u32).to_le_bytes());
        bytes.push(1);
        bytes.extend(value.as_bytes());
    }
    bytes
}

#[test]
fn reads_webkit_utf16_and_excludes_development_origins() {
    let root = tempfile::tempdir().unwrap();
    for (name, scheme, label) in [
        ("production", "tauri", "vorkspace 🦀"),
        ("development", "http", "Stale name"),
    ] {
        let path = root.path().join(name).join(name);
        std::fs::create_dir_all(path.join("LocalStorage")).unwrap();
        std::fs::write(path.join("origin"), origin(scheme, "localhost")).unwrap();
        let conn = Connection::open(path.join("LocalStorage/localstorage.sqlite3")).unwrap();
        conn.execute_batch("CREATE TABLE ItemTable(key TEXT,value BLOB)")
            .unwrap();
        let value: Vec<u8> = label.encode_utf16().flat_map(u16::to_le_bytes).collect();
        conn.execute(
            "INSERT INTO ItemTable VALUES(?1,?2)",
            rusqlite::params!["vorkspace.test", value],
        )
        .unwrap();
    }
    let preferences = DesktopPreferences::load_webkit(root.path());
    assert_eq!(preferences.get("vorkspace.test"), Some("vorkspace 🦀"));
    assert!(!desktop_origin(&origin("http", "localhost")));
    assert!(desktop_origin(&origin("https", "tauri.localhost")));
    assert!(!desktop_origin(b"tauri localhost"));
    assert_eq!(decode_webkit_value(&[0]), None);
}

#[test]
fn skips_empty_normalized_origins_and_keeps_the_source_unchanged() {
    let root = tempfile::tempdir().unwrap();
    // The first origin sorts first but carries no app-owned key after
    // compatibility normalization. It must not mask the later snapshot.
    let (empty, empty_bytes) =
        create_origin_under(root.path(), "a-empty", &[("unrelated.key", "value")]);
    let (populated, populated_bytes) = create_origin_under(
        root.path(),
        "b-populated",
        &[("vorkspace.modelControls", "beside")],
    );
    let preferences = DesktopPreferences::load_webkit(root.path());
    assert!(preferences.is_available());
    assert_eq!(preferences.get("vorkspace.modelControls"), Some("beside"));
    assert_eq!(std::fs::read(empty).unwrap(), empty_bytes);
    assert_eq!(std::fs::read(populated).unwrap(), populated_bytes);

    let all_empty = tempfile::tempdir().unwrap();
    let (path, bytes) = create_origin_under(all_empty.path(), "only-empty", &[]);
    let preferences = DesktopPreferences::load_webkit(all_empty.path());
    assert!(!preferences.is_available());
    let mut workspace = json!({
        "projects": ["/code/native"],
        "project": "/code/native",
        "preferences": {"model_controls": "beside"},
        "desktop_project_preferences": {"model_controls": "beside"}
    });
    let original = workspace.clone();
    preferences.apply_workspace(&mut workspace);
    assert_eq!(workspace, original);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

fn create_origin_under(
    root: &Path,
    name: &str,
    entries: &[(&str, &str)],
) -> (std::path::PathBuf, Vec<u8>) {
    let path = root
        .join(name)
        .join(name)
        .join("LocalStorage/localstorage.sqlite3");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path.parent().unwrap().parent().unwrap().join("origin"),
        origin("tauri", "localhost"),
    )
    .unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE ItemTable(key TEXT,value BLOB)")
        .unwrap();
    for (key, value) in entries {
        conn.execute(
            "INSERT INTO ItemTable VALUES(?1,?2)",
            rusqlite::params![key, value],
        )
        .unwrap();
    }
    drop(conn);
    (path.clone(), std::fs::read(path).unwrap())
}

#[test]
fn earlier_feature_keys_restore_in_place_and_keep_native_edits_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("sessions.db");
    let store = crate::store::Store::open_vorkspace(&database).unwrap();
    let session = store.create("codex", "/project").unwrap();
    let original = store.persisted_session_id(session.id).unwrap();
    drop(store);
    let app = String::from_utf8(vec![109, 111, 110, 111, 99, 111, 100, 101]).unwrap();
    let singular = String::from_utf8(vec![109, 111, 110, 111]).unwrap();
    let plural = format!("{singular}s");
    let storage = directory.path().join("webkit.sqlite3");
    let conn = Connection::open(&storage).unwrap();
    conn.execute_batch("CREATE TABLE ItemTable(key TEXT,value BLOB)")
        .unwrap();
    let mut channel = json!({"id":"channel","name":"team","topic":"saved","memberSessions":{"stable-bot":original},"future":{"keep":true}});
    channel[format!("{singular}Ids")] = json!(["stable-bot"]);
    let mut author = json!({"kind":singular,"future":7});
    author[format!("{singular}Id")] = json!("stable-bot");
    let mut reaction = json!({"emoji":"yes","future":8});
    reaction[format!("{singular}Id")] = json!("stable-bot");
    let values = [
            (format!("{app}:{singular}-roster"),json!([{"id":"stable-bot","name":"Before","sessionId":original,"projects":["/project"]}]).to_string()),
            (format!("{app}.{plural}Enabled"),"0".into()),
            (format!("{app}.rail{}{}Pinned",singular[..1].to_uppercase(),&plural[1..]),"1".into()),
            (format!("{app}.{singular}MenuBarIcon"),"0".into()),
            (format!("{app}:{singular}-channels"),json!([channel]).to_string()),
            (format!("{app}:{singular}-channel-messages:channel"),json!([{"id":"message","text":singular,"from":author,"reactions":[reaction],"future":9}]).to_string()),
            (format!("{app}:project-chat-backgrounds"),json!({format!("{singular}:stable-bot"):{"path":"/private/background.png","future":true}}).to_string()),
            (format!("{app}.keybindingOverrides"),json!({format!("App: Toggle {}{}",singular[..1].to_uppercase(),&singular[1..]):{"shortcut":"Command+KeyB"},"Future command":{"shortcut":"Command+KeyJ","future":true}}).to_string()),
            (format!("{app}.settingsSection"),plural.clone()),
        ];
    for (key, raw) in &values {
        let bytes: Vec<_> = raw.encode_utf16().flat_map(u16::to_le_bytes).collect();
        conn.execute(
            "INSERT INTO ItemTable VALUES(?1,?2)",
            rusqlite::params![key, bytes],
        )
        .unwrap();
    }
    drop(conn);
    let before = std::fs::read(&storage).unwrap();
    let preferences = std::sync::Arc::new(DesktopPreferences::read_database(&storage).unwrap());
    let mut workspace = json!({});
    preferences.apply_workspace(&mut workspace);
    assert_eq!(workspace["preferences"]["bots"], false);
    assert_eq!(workspace["preferences"]["rail_bots_pinned"], true);
    assert_eq!(workspace["preferences"]["bot_menu_bar_icon"], false);
    assert_eq!(
        workspace["preferences"]["keybindings"]["App: Toggle Bot"],
        "cmd-b"
    );
    assert_eq!(preferences.get("vorkspace.settingsSection"), Some("bots"));
    let store = crate::store::Store::open_vorkspace_with_preferences_for_test(
        &database,
        preferences.clone(),
    )
    .unwrap();
    let mut roster = crate::bot::Roster::load(&store).unwrap();
    assert_eq!(roster.bots[0].session_id, Some(session.id));
    assert_eq!(
        roster.bots[0].background.as_ref().unwrap().path,
        "/private/background.png"
    );
    roster.bots[0].name = "After".into();
    roster.save(&store).unwrap();
    let channels = crate::bot::channels::Channels::load(&store).unwrap();
    assert_eq!(channels.channels[0].member_ids, ["stable-bot"]);
    assert_eq!(
        channels.channels[0].messages[0]["from"]["botId"],
        "stable-bot"
    );
    assert_eq!(channels.channels[0].messages[0]["from"]["kind"], "bot");
    assert_eq!(channels.channels[0].messages[0]["text"], singular);
    assert_eq!(
        channels.channels[0].messages[0]["reactions"][0]["future"],
        8
    );
    assert_eq!(channels.channels[0].extra["future"]["keep"], true);
    channels.save(&store).unwrap();
    drop(store);
    let reopened =
        crate::store::Store::open_vorkspace_with_preferences_for_test(&database, preferences)
            .unwrap();
    assert_eq!(
        crate::bot::Roster::load(&reopened).unwrap().bots[0].name,
        "After"
    );
    assert_eq!(
        crate::bot::channels::Channels::load(&reopened)
            .unwrap()
            .channels[0]
            .messages[0]["future"],
        9
    );
    assert_eq!(std::fs::read(&storage).unwrap(), before);
}

#[test]
fn canonical_preferences_replace_stale_sidecar_projects_and_appearance() {
    let preferences = prefs(&[
        (
            "vorkspace.recentProjects",
            json!([{"path":"/code/agent-terminal"},{"path":"/code/noodle"},{"path":"/code/archived"}]),
        ),
        (
            "vorkspace.projectRailOrder",
            json!(["/code/noodle", "/code/agent-terminal"]),
        ),
        (
            "vorkspace.projectRailPinned",
            json!(["/code/agent-terminal"]),
        ),
        (
            "vorkspace.archivedProjects",
            json!([{"path":"/code/archived"}]),
        ),
        (
            "vorkspace:tab-group:labels",
            json!({"/code/agent-terminal":"vorkspace"}),
        ),
        (
            "vorkspace:tab-group:logos",
            json!({"/code/agent-terminal":"/logos/saved.png"}),
        ),
    ]);
    let mut workspace = json!({"projects":["/tmp/bot","/code/agent-terminal"],"project":"/tmp/bot","active":123,
            "project_appearance":{"/code/agent-terminal":{"name":"stale","background":{"path":"/background.png"}}}});
    preferences.apply_workspace(&mut workspace);
    assert_eq!(
        workspace["projects"],
        json!(["/code/agent-terminal", "/code/noodle"])
    );
    assert_eq!(workspace["project"], "/code/agent-terminal");
    assert!(workspace["active"].is_null());
    assert_eq!(
        workspace["project_appearance"]["/code/agent-terminal"]["name"],
        "vorkspace"
    );
    assert_eq!(
        workspace["project_appearance"]["/code/agent-terminal"]["logo"],
        "/logos/saved.png"
    );
    assert_eq!(
        workspace["project_appearance"]["/code/agent-terminal"]["background"]["path"],
        "/background.png"
    );
}

#[test]
fn archived_project_restore_survives_stale_and_missing_source() {
    let source = prefs(&[
        (
            "vorkspace.recentProjects",
            json!([{"path":"C:/Work/Repo/"}]),
        ),
        (
            "vorkspace.archivedProjects",
            json!([{"path":"c:\\work\\repo"}]),
        ),
    ]);
    let mut workspace = json!({});
    source.apply_workspace(&mut workspace);
    assert_eq!(workspace["project_archive"]["c:/work/repo"], true);
    assert_eq!(workspace["projects"], json!([]));
    workspace["project_archive"]["c:/work/repo"] = json!(false);
    source.apply_workspace(&mut workspace);
    assert_eq!(workspace["projects"], json!(["c:/work/repo"]));
    let saved = serde_json::to_string(&workspace).unwrap();
    let mut reopened: Value = serde_json::from_str(&saved).unwrap();
    DesktopPreferences {
        values: Default::default(),
        available: false,
    }
    .apply_workspace(&mut reopened);
    source.apply_workspace(&mut reopened);
    assert_eq!(reopened["project_archive"]["c:/work/repo"], false);
    assert_eq!(reopened["projects"], json!(["c:/work/repo"]));
}

#[test]
fn native_deleted_project_survives_first_and_later_desktop_snapshots() {
    let source = prefs(&[(
        "vorkspace.recentProjects",
        json!([{"path":"/code/gone"}, {"path":"/code/keep"}]),
    )]);
    let mut state = json!({"projects":["/code/keep"], "project":"/code/keep", "deleted_projects":["/code/gone"]});
    source.apply_workspace(&mut state);
    assert_eq!(state["projects"], json!(["/code/keep"]));
    source.apply_workspace(&mut state);
    assert_eq!(state["projects"], json!(["/code/keep"]));
    state["deleted_projects"] = json!([]);
    state["projects"] = json!(["/code/gone", "/code/keep"]);
    source.apply_workspace(&mut state);
    assert_eq!(state["projects"], json!(["/code/gone", "/code/keep"]));
}

#[test]
fn deleted_project_is_not_resurrected_by_an_older_archive_restore() {
    let source = prefs(&[("vorkspace.recentProjects", json!([{"path":"/code/gone"}]))]);
    let mut state = json!({"projects":[], "project_archive":{"/code/gone":false}, "deleted_projects":["/code/gone"]});
    source.apply_workspace(&mut state);
    assert_eq!(state["projects"], json!([]));
    state["deleted_projects"] = json!([]);
    source.apply_workspace(&mut state);
    assert_eq!(state["projects"], json!(["/code/gone"]));
}

#[test]
fn native_edits_survive_until_the_corresponding_desktop_setting_changes() {
    let initial = prefs(&[
        (
            "vorkspace.recentProjects",
            json!([{"path":"/code/a"},{"path":"/code/b"}]),
        ),
        ("vorkspace:tab-group:labels", json!({"/code/a":"Desktop"})),
    ]);
    let mut workspace = json!({});
    initial.apply_workspace(&mut workspace);
    workspace["projects"] = json!(["/code/a", "/code/native"]);
    workspace["project_appearance"]["/code/a"]["name"] = json!("Native");
    workspace["project_appearance"]["/code/a"]["mascot"] = json!("cat");
    initial.apply_workspace(&mut workspace);
    assert_eq!(workspace["projects"], json!(["/code/a", "/code/native"]));
    assert_eq!(workspace["project_appearance"]["/code/a"]["name"], "Native");
    assert_eq!(workspace["project_appearance"]["/code/a"]["mascot"], "cat");
    let changed = prefs(&[
        (
            "vorkspace.recentProjects",
            json!([{"path":"/code/a"},{"path":"/code/new"}]),
        ),
        (
            "vorkspace:tab-group:labels",
            json!({"/code/a":"Renamed in desktop"}),
        ),
    ]);
    changed.apply_workspace(&mut workspace);
    assert_eq!(
        workspace["projects"],
        json!(["/code/a", "/code/new", "/code/native"])
    );
    assert_eq!(
        workspace["project_appearance"]["/code/a"]["name"],
        "Renamed in desktop"
    );
    assert_eq!(workspace["project_appearance"]["/code/a"]["mascot"], "cat");
}

#[test]
fn model_control_placement_matches_desktop_and_retains_explicit_native_edits() {
    let beside = DesktopPreferences {
        available: true,
        values: BTreeMap::from([("vorkspace.modelControls".into(), "beside".into())]),
    };
    let mut workspace = json!({"preferences":{"model_controls":"menu"}});
    beside.apply_workspace(&mut workspace);
    assert_eq!(workspace["preferences"]["model_controls"], "beside");
    workspace["preferences"]["model_controls"] = json!("menu");
    beside.apply_workspace(&mut workspace);
    assert_eq!(workspace["preferences"]["model_controls"], "menu");
    let defaults = DesktopPreferences {
        available: true,
        ..Default::default()
    };
    defaults.apply_workspace(&mut workspace);
    assert_eq!(workspace["preferences"]["model_controls"], "menu");
    let legacy = DesktopPreferences {
        available: true,
        values: BTreeMap::from([("vorkspace.composerEffortVisible".into(), "1".into())]),
    };
    legacy.apply_workspace(&mut workspace);
    assert_eq!(workspace["preferences"]["model_controls"], "beside");
}

#[test]
fn desktop_provider_choices_seed_defaults_and_preserve_explicit_native_edits() {
    let source = prefs(&[
        (
            "vorkspace.lastModel",
            json!({"harness":"claude","model":"claude:opus-4.8"}),
        ),
        (
            "vorkspace.projectProviderSettings.v1",
            json!({"/project":{"defaultHarness":"codex"}}),
        ),
    ]);
    let mut workspace = json!({});
    source.apply_workspace(&mut workspace);
    assert_eq!(workspace["preferences"]["default_provider"], "claude");
    workspace["preferences"]["default_provider"] = json!("codex");
    source.apply_workspace(&mut workspace);
    assert_eq!(workspace["preferences"]["default_provider"], "codex");
    // Removing a desktop override clears its old source-backed value.
    prefs(&[]).apply_workspace(&mut workspace);
    assert!(workspace["preferences"]["default_provider"].is_null());
}

#[test]
fn unavailable_preferences_preserve_the_entire_workspace() {
    let mut workspace = json!({
        "projects": ["/code/native"],
        "project_appearance": {"/code/a":{"name":"Saved"}},
        "desktop_project_preferences": {"appearance":{"/code/a":{"name":"Saved"}}}
    });
    let original = workspace.clone();
    DesktopPreferences::default().apply_workspace(&mut workspace);
    assert_eq!(workspace, original);
}

#[test]
fn isolated_data_directory_cannot_read_desktop_preferences() {
    let directory = tempfile::tempdir().unwrap();
    assert!(DesktopPreferences::load(directory.path()).values.is_empty());
}
