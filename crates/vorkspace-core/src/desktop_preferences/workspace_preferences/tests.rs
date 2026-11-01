
use super::*;

fn source(entries: &[(&str, &str)]) -> DesktopPreferences {
    DesktopPreferences {
        available: true,
        values: entries
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    }
}

#[test]
fn voice_controls_read_existing_settings_and_preserve_later_native_choices() {
    let preferences = source(&[
        ("vorkspace.vox.enabled", "1"),
        ("vorkspace.vox.silent", "true"),
        (
            "vorkspace.vox.microphone.v1",
            r#"{"id":"saved-device","label":"USB microphone"}"#,
        ),
    ]);
    let mut workspace = json!({});
    preferences.apply_workspace(&mut workspace);
    assert_eq!(workspace["preferences"]["vox_enabled"], true);
    assert_eq!(workspace["preferences"]["vox_silent"], true);
    assert_eq!(
        workspace["preferences"]["vox_microphone"]["id"],
        "saved-device"
    );
    workspace["preferences"]["vox_enabled"] = json!(false);
    workspace["preferences"]["vox_microphone"] =
        json!({"id":"builtin","label":"built-in microphone"});
    preferences.apply_workspace(&mut workspace);
    assert_eq!(workspace["preferences"]["vox_enabled"], false);
    assert_eq!(workspace["preferences"]["vox_microphone"]["id"], "builtin");

    let mut empty = json!({});
    source(&[("vorkspace.vox.microphone.v1", r#"{"id":"","label":7}"#)])
        .apply_workspace(&mut empty);
    assert_eq!(empty["preferences"]["vox_enabled"], false);
    assert_eq!(empty["preferences"]["vox_microphone"]["id"], "builtin");
}

#[test]
fn desktop_settings_and_sidebar_filters_use_native_names_units_and_enums() {
    let preferences = source(&[
        ("vorkspace.sounds", "0"),
        ("vorkspace.colorScheme", "light"),
        ("vorkspace.workspaceTheme", "tidepool"),
        ("vorkspace.insetLayout", "1"),
        ("vorkspace.sessionSidebarOpen", "0"),
        ("vorkspace.projectRailOpen", "1"),
        ("vorkspace.sidebarOpacity", "0.42"),
        ("vorkspace.uiScale", "1.4"),
        ("vorkspace.chatBackgroundSessionOpacity", "0.31"),
        (
            "vorkspace.rightPanelTabOrder",
            "[\"changes\",\"files\",\"changes\"]",
        ),
        ("vorkspace.changesView", "tree"),
        (
            "vorkspace.sessionSidebarFilters",
            r#"{"showArchived":true,"hiddenHarnesses":["codex","unknown","codex"],"time":"7d","status":{"needsApproval":true}}"#,
        ),
        (
            "vorkspace.defaultModels",
            r#"{"codex":"codex:gpt-6.1-sol","claude":"claude:opus-4.8"}"#,
        ),
        (
            "vorkspace.keybindingOverrides",
            r#"{"App: Search":{"shortcut":"Command+KeyJ"},"App: Settings":{"disabled":true}}"#,
        ),
    ]);
    let mut workspace = json!({});
    preferences.apply_workspace(&mut workspace);
    let settings = &workspace["preferences"];
    assert_eq!(settings["sounds"], false);
    assert_eq!(settings["theme"], "light");
    assert_eq!(settings["workspace_theme"], "tidepool");
    assert_eq!(settings["sidebar_opacity"], 42);
    assert_eq!(settings["ui_scale"], 140);
    assert_eq!(settings["chat_background_session"], 31);
    assert_eq!(settings["show_archived"], true);
    assert_eq!(settings["default_models"]["codex"], "gpt-6.1-sol");
    assert_eq!(settings["default_models"]["claude"], "claude-opus-4-8");
    assert_eq!(settings["keybindings"]["App: Search"], "cmd-j");
    assert!(settings["keybindings"]["App: Settings"].is_null());
    assert_eq!(workspace["classic_layout"], true);
    assert_eq!(workspace["sidebar_visible"], false);
    assert_eq!(
        workspace["right_panel_tabs"],
        json!(["changes", "files", "graph"])
    );
    assert_eq!(workspace["changes_view"], "tree");
    let filters: crate::session_filters::Filters =
        serde_json::from_value(workspace["session_filters"].clone()).unwrap();
    assert_eq!(filters.hidden_providers, ["codex"]);
    assert_eq!(filters.time, crate::session_filters::TimeFilter::Week);
    assert!(filters.status.needs_approval);
}

#[test]
fn unrelated_desktop_map_edits_preserve_native_model_choices_and_added_keys() {
    let initial = source(&[(
        "vorkspace.defaultModels",
        r#"{"codex":"codex:gpt-old","claude":"claude:opus-4.8"}"#,
    )]);
    let mut workspace = json!({});
    initial.apply_workspace(&mut workspace);
    workspace["preferences"]["default_models"]["codex"] = json!("gpt-native");
    workspace["preferences"]["default_models"]["cursor"] = json!("native-added");
    workspace["preferences"]["sounds"] = json!(false);
    let changed = source(&[(
        "vorkspace.defaultModels",
        r#"{"codex":"codex:gpt-old","claude":"claude:sonnet-4.8"}"#,
    )]);
    changed.apply_workspace(&mut workspace);
    assert_eq!(
        workspace["preferences"]["default_models"]["codex"],
        "gpt-native"
    );
    assert_eq!(
        workspace["preferences"]["default_models"]["cursor"],
        "native-added"
    );
    assert_eq!(
        workspace["preferences"]["default_models"]["claude"],
        "claude-sonnet-4-8"
    );
    assert_eq!(workspace["preferences"]["sounds"], false);
}

#[test]
fn project_backgrounds_keep_independent_opacities_and_native_overrides() {
    let source = source(&[(
        "vorkspace:project-chat-backgrounds",
        r#"{"/work":{"path":"/images/background.png","emptyOpacity":0.2,"sessionOpacity":0.4,"scope":"empty","effect":"ascii"}}"#,
    )]);
    let mut workspace = json!({});
    source.apply_workspace(&mut workspace);
    let appearance: crate::project_appearance::ProjectAppearance =
        serde_json::from_value(workspace["project_appearance"]["/work"].clone()).unwrap();
    let background = appearance.background.unwrap();
    assert!((background.empty_opacity - 0.2).abs() < 1e-6);
    assert!((background.session_opacity - 0.4).abs() < 1e-6);
    assert_eq!(background.scope, crate::project_appearance::Scope::Empty);
    assert_eq!(background.effect, crate::project_appearance::Effect::Ascii);
    workspace["project_appearance"]["/work"]["background"]["empty_opacity"] = json!(0.3);
    source.apply_workspace(&mut workspace);
    assert_eq!(
        workspace["project_appearance"]["/work"]["background"]["empty_opacity"],
        0.3
    );
}

#[test]
fn desktop_chords_translate_without_interpreting_unknown_codes() {
    assert_eq!(
        shortcut("Command+Shift+KeyP").as_deref(),
        Some("cmd-shift-p")
    );
    assert_eq!(
        shortcut("Control+Option+ArrowLeft").as_deref(),
        Some("ctrl-alt-left")
    );
    assert!(shortcut("Command+Invalid").is_none());
}

#[test]
fn source_picker_choices_and_file_mode_reconcile_and_unported_prefs_stay_exact() {
    let source = source(&[
        (
            "vorkspace.favoriteModels",
            r#"["claude:opus-4.8","codex:gpt-6.1-sol","claude:opus-4.8","unknown:model"]"#,
        ),
        (
            "vorkspace.recentModels",
            r#"[{"harness":"codex","model":"codex:gpt-6.1-sol"},{"harness":"claude","model":"claude:opus-4.8"}]"#,
        ),
        ("vorkspace.modelPickerTab", "codex"),
        ("vorkspace.fileViewMode", "source"),
        ("vorkspace.projectSidebarTabs.v1", r#"{"/work":"sessions"}"#),
    ]);
    let mut workspace = json!({});
    source.apply_workspace(&mut workspace);
    assert_eq!(
        workspace["preferences"]["favorite_models"],
        json!([
            {"provider":"claude", "model":"claude-opus-4-8"},
            {"provider":"codex", "model":"gpt-6.1-sol"}
        ])
    );
    assert_eq!(
        workspace["preferences"]["recent_models"][0]["provider"],
        "codex"
    );
    assert_eq!(workspace["preferences"]["model_picker_tab"], "codex");
    assert_eq!(workspace["preferences"]["file_view_mode"], "source");
    assert_eq!(
        workspace["desktop_preferences_metadata"]["vorkspace.projectSidebarTabs.v1"],
        r#"{"/work":"sessions"}"#
    );
    workspace["preferences"]["model_picker_tab"] = json!("favorites");
    workspace["preferences"]["file_view_mode"] = json!("preview");
    source.apply_workspace(&mut workspace);
    assert_eq!(workspace["preferences"]["model_picker_tab"], "favorites");
    assert_eq!(workspace["preferences"]["file_view_mode"], "preview");
}

#[test]
fn absent_desktop_inset_setting_uses_the_desktop_classic_default() {
    let mut workspace = json!({});
    source(&[]).apply_workspace(&mut workspace);
    assert_eq!(workspace["classic_layout"], true);
    let mut isolated = json!({"classic_layout":false});
    DesktopPreferences::default().apply_workspace(&mut isolated);
    assert_eq!(isolated["classic_layout"], false);
}

#[test]
fn classic_sidebar_preferences_survive_native_edits_save_and_desktop_reread() {
    let initial = source(&[
        ("vorkspace.projectRailOpen", "0"),
        ("vorkspace.projectRailWidth", "900"),
        (
            "vorkspace.projectSidebarTabs.v1",
            r#"{"/work":"files","/other":"sessions","/bad":"other","/graph":"graph"}"#,
        ),
        (
            "vorkspace.sidebarTabOrder",
            r#"["changes","inbox","changes","sessions"]"#,
        ),
    ]);
    let mut workspace = json!({
        "project":"/work", "draft":"keep", "project_group_collapsed":{"group":true}
    });
    initial.apply_workspace(&mut workspace);
    assert_eq!(workspace["project_rail_open"], false);
    assert_eq!(workspace["project_rail_width"], 360.);
    assert_eq!(
        workspace["sidebar_tabs"],
        json!({"/work":"files","/other":"sessions"})
    );
    assert_eq!(
        workspace["sidebar_tab_order"],
        json!(["changes", "sessions", "files"])
    );

    workspace["project_rail_open"] = json!(true);
    workspace["project_rail_width"] = json!(245.);
    workspace["sidebar_tabs"]["/work"] = json!("changes");
    workspace["sidebar_tabs"]["/native"] = json!("graph");
    workspace["sidebar_tab_order"] = json!(["files", "changes", "sessions"]);
    let saved = serde_json::to_string(&workspace).unwrap();
    let mut reopened: Value = serde_json::from_str(&saved).unwrap();
    initial.apply_workspace(&mut reopened);
    assert_eq!(reopened, workspace);

    let mut changed = initial.clone();
    changed.values.insert(
        "vorkspace.projectSidebarTabs.v1".into(),
        r#"{"/work":"files","/other":"changes"}"#.into(),
    );
    changed.apply_workspace(&mut reopened);
    assert_eq!(reopened["sidebar_tabs"]["/work"], "changes");
    assert_eq!(reopened["sidebar_tabs"]["/native"], "graph");
    assert_eq!(reopened["sidebar_tabs"]["/other"], "changes");
    assert_eq!(reopened["project_rail_width"], 245.);
    assert_eq!(reopened["draft"], "keep");
    assert_eq!(reopened["project_group_collapsed"]["group"], true);
    assert_eq!(
        reopened["desktop_preferences_metadata"]["vorkspace.sidebarTabOrder"],
        r#"["changes","inbox","changes","sessions"]"#
    );
    changed
        .values
        .insert("vorkspace.projectRailWidth".into(), "275".into());
    changed.apply_workspace(&mut reopened);
    assert_eq!(reopened["project_rail_width"], 275.);
}

#[test]
fn new_sidebar_source_baselines_preserve_existing_canonical_native_choices() {
    let mut workspace = json!({
        "classic_layout":false,
        "project_rail_open":false,
        "project_rail_width":245.,
        "sidebar_tabs":{"/work":"graph"},
        "sidebar_tab_order":["changes","files","sessions"],
        "desktop_project_preferences":{"workspace":{"classic_layout":false}}
    });
    let preferences = source(&[
        ("vorkspace.insetLayout", "1"),
        ("vorkspace.projectRailOpen", "1"),
        ("vorkspace.projectRailWidth", "180"),
        ("vorkspace.projectSidebarTabs.v1", r#"{"/work":"files"}"#),
        ("vorkspace.sidebarTabOrder", r#"["sessions"]"#),
    ]);
    preferences.apply_workspace(&mut workspace);
    assert_eq!(workspace["classic_layout"], true);
    assert_eq!(workspace["project_rail_open"], false);
    assert_eq!(workspace["project_rail_width"], 245.);
    assert_eq!(workspace["sidebar_tabs"]["/work"], "graph");
    assert_eq!(
        workspace["sidebar_tab_order"],
        json!(["changes", "files", "sessions"])
    );
    let saved = workspace.clone();
    preferences.apply_workspace(&mut workspace);
    assert_eq!(workspace, saved);
}

#[test]
fn sidebar_preferences_survive_actual_store_save_close_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vorkspace.db");
    let preferences = std::sync::Arc::new(source(&[
        ("vorkspace.projectRailOpen", "0"),
        ("vorkspace.projectRailWidth", "215"),
        ("vorkspace.projectSidebarTabs.v1", r#"{"/work":"files"}"#),
        ("vorkspace.projectRailPinned", r#"["/work"]"#),
    ]));
    let store =
        crate::store::Store::open_vorkspace_with_preferences_for_test(&path, preferences.clone())
            .unwrap();
    let session = store.create("codex", "/work").unwrap();
    let persisted_session = store.persisted_session_id(session.id).unwrap();
    let mut workspace: Value = serde_json::from_str(&store.ui_state().unwrap().unwrap()).unwrap();
    workspace["project_rail_open"] = json!(true);
    workspace["project_rail_width"] = json!(285.);
    workspace["sidebar_tabs"]["/work"] = json!("changes");
    workspace["sidebar_tab_order"] = json!(["files", "sessions", "changes"]);
    workspace["project_group_collapsed"] = json!({"group":true});
    store.save_ui_state(&workspace.to_string()).unwrap();
    drop(store);

    let reopened =
        crate::store::Store::open_vorkspace_with_preferences_for_test(&path, preferences).unwrap();
    let actual: Value = serde_json::from_str(&reopened.ui_state().unwrap().unwrap()).unwrap();
    assert_eq!(actual, workspace);
    assert_eq!(
        actual["desktop_preferences_metadata"]["vorkspace.projectRailPinned"],
        r#"["/work"]"#
    );
    assert_eq!(
        reopened
            .persisted_session_id(reopened.sessions().unwrap()[0].id)
            .unwrap(),
        persisted_session
    );
}

#[test]
fn first_native_sidebar_schema_recovers_retained_source_without_webkit() {
    let metadata = json!({
        "vorkspace.projectRailOpen":"false",
        "vorkspace.projectRailWidth":"234",
        "vorkspace.projectSidebarTabs.v1":r#"{"/work":"changes"}"#,
        "vorkspace.sidebarTabOrder":r#"["files","changes"]"#,
        "opaque":"kept"
    });
    let original = json!({
        "project":"/work",
        "preferences":{"theme":"light"},
        "desktop_project_preferences":{"workspace":{"classic_layout":true}},
        "desktop_preferences_metadata":metadata
    });
    let mut restored = original.clone();
    DesktopPreferences::restore_sidebar_preferences(&mut restored);
    assert_eq!(restored["project_rail_open"], false);
    assert_eq!(restored["project_rail_width"], 234.);
    assert_eq!(restored["sidebar_tabs"]["/work"], "changes");
    assert_eq!(
        restored["sidebar_tab_order"],
        json!(["files", "changes", "sessions"])
    );
    assert_eq!(restored["desktop_preferences_metadata"], metadata);
    assert_eq!(restored["preferences"], original["preferences"]);
    restored["project_rail_width"] = json!(260.);
    restored["sidebar_tabs"]["/work"] = json!("sessions");
    DesktopPreferences::restore_sidebar_preferences(&mut restored);
    assert_eq!(restored["project_rail_width"], 260.);
    assert_eq!(restored["sidebar_tabs"]["/work"], "sessions");

    let mut available_but_missing = original;
    source(&[]).apply_workspace(&mut available_but_missing);
    assert_eq!(available_but_missing["project_rail_width"], 234.);
    assert_eq!(available_but_missing["sidebar_tabs"]["/work"], "changes");
    assert_eq!(
        available_but_missing["desktop_preferences_metadata"],
        metadata
    );
    source(&[]).apply_workspace(&mut available_but_missing);
    assert_eq!(available_but_missing["project_rail_width"], 234.);
}

#[test]
fn malformed_sidebar_source_uses_defaults_and_does_not_clobber_other_fields() {
    let mut workspace = json!({"project":"/work", "draft":"keep"});
    source(&[
        ("vorkspace.projectRailOpen", "invalid"),
        ("vorkspace.projectRailWidth", "NaN"),
        (
            "vorkspace.projectSidebarTabs.v1",
            r#"{"/work":2,"/bad":"other","":"files","~":"changes"}"#,
        ),
        ("vorkspace.sidebarTabOrder", r#"[null,4,"inbox","graph"]"#),
    ])
    .apply_workspace(&mut workspace);
    assert_eq!(workspace["classic_layout"], true);
    assert_eq!(workspace["project_rail_open"], true);
    assert_eq!(workspace["project_rail_width"], 200.);
    assert_eq!(workspace["sidebar_tabs"], json!({}));
    assert_eq!(
        workspace["sidebar_tab_order"],
        json!(["sessions", "files", "changes"])
    );
    assert_eq!(workspace["draft"], "keep");
    workspace["project_rail_open"] = json!("bad");
    workspace["project_rail_width"] = json!(-10.);
    workspace["sidebar_tabs"] = json!({"/work":"graph","/bad":4});
    workspace["sidebar_tab_order"] = json!(["graph", "files", "files", 4]);
    DesktopPreferences::restore_sidebar_preferences(&mut workspace);
    assert_eq!(workspace["project_rail_open"], true);
    assert_eq!(workspace["project_rail_width"], 180.);
    assert_eq!(workspace["sidebar_tabs"], json!({"/work":"graph"}));
    assert_eq!(
        workspace["sidebar_tab_order"],
        json!(["files", "sessions", "changes"])
    );
    assert_eq!(workspace["draft"], "keep");
}
