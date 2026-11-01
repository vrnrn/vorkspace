
use super::*;

#[test]
fn parses_single_server_from_standard_json() {
    let (name, server) = server_from_json(
        "claude",
        "",
        r#"{"mcpServers":{"docs":{"command":"npx","args":["server"]}}}"#,
    )
    .unwrap();
    assert_eq!(name, "docs");
    assert_eq!(server["command"], "npx");
    assert!(server_from_json(
        "claude",
        "",
        r#"{"mcpServers":{"one":{"command":"npx"},"two":{"command":"node"}}}"#
    )
    .is_err());
    assert!(server_from_json(
        "claude",
        "different",
        r#"{"mcpServers":{"docs":{"command":"npx"}}}"#
    )
    .is_err());
}

#[test]
fn concurrent_additions_keep_every_server() {
    let root = std::env::temp_dir().join(format!("vorkspace-mcp-lock-{}", rand::random::<u128>()));
    let path = root.join("claude_desktop_config.json");
    std::thread::scope(|scope| {
        for index in 0..8 {
            let path = path.clone();
            scope.spawn(move || {
                write_json_server(
                    &path,
                    &format!("server-{index}"),
                    serde_json::json!({"command":"npx"}),
                )
                .unwrap();
            });
        }
    });
    let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["mcpServers"].as_object().unwrap().len(), 8);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replacing_config_restricts_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("vorkspace-mcp-mode-{}", rand::random::<u128>()));
    let path = root.join("mcp.json");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&path, r#"{"mcpServers":{}}"#).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    write_json_server(&path, "new", serde_json::json!({"command":"npx"})).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn writes_server_without_discarding_other_configuration() {
    let root = std::env::temp_dir().join(format!("vorkspace-mcp-write-{}", rand::random::<u128>()));
    let path = root.join("claude_desktop_config.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"otherSetting":true,"mcpServers":{"existing":{"command":"node"}}}"#,
    )
    .unwrap();
    write_json_server(&path, "new", serde_json::json!({"command":"npx"})).unwrap();
    let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["otherSetting"], true);
    assert_eq!(value["mcpServers"]["existing"]["command"], "node");
    assert_eq!(value["mcpServers"]["new"]["command"], "npx");
    assert!(write_json_server(&path, "new", serde_json::json!({"command":"npx"})).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn adds_to_existing_desktop_jsonc() {
    let root = std::env::temp_dir().join(format!("vorkspace-mcp-jsonc-{}", rand::random::<u128>()));
    let path = root.join("claude_desktop_config.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "{\n // existing server\n \"mcpServers\": {\"old\": {\"command\": \"node\",},},\n}",
    )
    .unwrap();
    write_json_server(&path, "new", serde_json::json!({"command":"npx"})).unwrap();
    let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["mcpServers"]["old"]["command"], "node");
    assert_eq!(value["mcpServers"]["new"]["command"], "npx");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn jsonc_preserves_urls_and_removes_comments_and_trailing_commas() {
    let input = r#"{"mcp":{"servers":{"docs":{"url":"https://example.com/mcp",},},}, // comment
        }"#;
    let value: Value = serde_json::from_str(&strip_jsonc(input)).unwrap();
    assert_eq!(
        value["mcp"]["servers"]["docs"]["url"],
        "https://example.com/mcp"
    );
}

#[test]
fn discovers_provider_configs_without_exposing_credentials() {
    let root = std::env::temp_dir().join(format!("vorkspace-mcp-{}", rand::random::<u128>()));
    let home = root.join("home");
    let project = root.join("project");
    std::fs::create_dir_all(home.join(".cursor")).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::create_dir_all(home.join(".config/opencode")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    // Stop the ancestor walk inside the fixture so real user configs (which
    // live in ancestors of the temp dir on a dev machine) are never read.
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(home.join(".claude.json"), r#"{"mcpServers":{"one":{"type":"http","url":"https://example.com","headers":{"Authorization":"secret"}}}}"#).unwrap();
    std::fs::write(
        home.join(".cursor/mcp.json"),
        r#"{"mcpServers":{"two":{"command":"npx"}}}"#,
    )
    .unwrap();
    let desktop = root.join("claude_desktop_config.json");
    std::fs::create_dir_all(desktop.parent().unwrap()).unwrap();
    std::fs::write(&desktop, r#"{"mcpServers":{"desktop":{"command":"npx"}}}"#).unwrap();
    std::fs::write(
        home.join(".codex/config.toml"),
        "[mcp_servers.three]\nurl = 'https://example.com'\n",
    )
    .unwrap();
    let codex_raw = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    let codex_config: toml::Value = toml::from_str(&codex_raw).unwrap();
    assert!(codex_config.get("mcp_servers").is_some());
    std::fs::write(home.join(".config/opencode/opencode.jsonc"), "{\"mcp\": {\"servers\": {\"four\": {\"type\": \"remote\", \"url\": \"https://example.com\",},},}}").unwrap();
    let found = discover(&home, &project, None, &desktop);
    let names: Vec<_> = found
        .iter()
        .map(|entry| (entry.provider.as_str(), entry.name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("claude", "one"),
            ("claude_desktop", "desktop"),
            ("codex", "three")
        ]
    );
    assert!(!serde_json::to_string(&found).unwrap().contains("secret"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovery_preserves_codex_enablement() {
    let root =
        std::env::temp_dir().join(format!("vorkspace-mcp-enabled-{}", rand::random::<u128>()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("config.toml");
    std::fs::write(
            &path,
            "[mcp_servers.off]\ncommand = 'node'\nenabled = false\n[mcp_servers.on]\ncommand = 'node'\n",
        ).unwrap();
    let mut found = Vec::new();
    add_toml_file(&mut found, "codex", "user", &path);
    assert_eq!(found.len(), 2);
    assert!(
        !found
            .iter()
            .find(|server| server.name == "off")
            .unwrap()
            .enabled
    );
    assert!(
        found
            .iter()
            .find(|server| server.name == "on")
            .unwrap()
            .enabled
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn does_not_duplicate_home_configs_or_hide_a_server_named_servers() {
    let root = std::env::temp_dir().join(format!("vorkspace-mcp-{}", rand::random::<u128>()));
    let project = root.join("work/notes");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        root.join(".claude.json"),
        r#"{"mcpServers":{"servers":{"command":"npx"},"docs":{"command":"node"}}}"#,
    )
    .unwrap();
    let found = discover(&root, &project, None, &root.join("desktop.json"));
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|server| server.scope == "user"));
    assert!(found.iter().any(|server| server.name == "servers"));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn retired_mcp_integrations_cannot_resolve_a_binary_or_accept_configuration() {
    for provider in ["cursor", "opencode", "grok", "fx", "hermes", "devin"] {
        assert!(
            server_from_json(provider, "server", r#"{"command":"node"}"#)
                .unwrap_err()
                .contains("Unsupported MCP provider")
        );
        assert!(resolve_mcp_binary(provider, Some("/unused/custom-cli"))
            .unwrap_err()
            .contains("Unsupported MCP provider"));
        assert!(login("/unused/project", provider, "server")
            .unwrap_err()
            .contains("Unsupported MCP provider"));
        assert!(mcp_add(
            "/unused/project".into(),
            provider.into(),
            "user".into(),
            "server".into(),
            r#"{"command":"node"}"#.into()
        )
        .unwrap_err()
        .contains("Unsupported MCP provider"));
    }
}
