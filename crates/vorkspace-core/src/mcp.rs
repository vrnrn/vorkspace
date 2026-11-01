use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::scm::shell::dirs_home;
fn expand_home(path: &str) -> PathBuf {
    crate::scm::expand_home(Path::new(path))
}

fn claude_desktop_config(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Claude/claude_desktop_config.json")
}

fn server_from_json(provider: &str, name: &str, config: &str) -> Result<(String, Value), String> {
    if !matches!(provider, "claude" | "codex" | "claude_desktop") {
        return Err("Unsupported MCP provider".into());
    }
    let value: Value = serde_json::from_str(config).map_err(|e| format!("Invalid JSON: {e}"))?;
    let (name, server) = if let Some(servers) = value.get("mcpServers") {
        let servers = servers.as_object().ok_or("mcpServers must be an object")?;
        if servers.len() != 1 {
            return Err("Add one server at a time".into());
        }
        let (server_name, server) = servers.iter().next().unwrap();
        if !name.trim().is_empty() && name.trim() != server_name {
            return Err("Name does not match the mcpServers entry".into());
        }
        (server_name.clone(), server.clone())
    } else {
        (name.trim().to_owned(), value)
    };
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("Server name cannot be empty or contain control characters".into());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Server name must use letters, numbers, hyphens, or underscores".into());
    }
    if !server.is_object() {
        return Err("Server configuration must be an object".into());
    }
    let command = server
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    let url = server
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    if command == url {
        return Err("Server needs either a command or a URL".into());
    }
    Ok((name, server))
}

pub fn mcp_add(
    cwd: String,
    provider: String,
    scope: String,
    name: String,
    config: String,
) -> Result<(), String> {
    let (name, server) = server_from_json(&provider, &name, &config)?;
    let binary_path = crate::provider::resolve(crate::provider::Kind::from_id(&provider))
        .map(|p| p.to_string_lossy().into_owned());
    {
        let home = dirs_home().ok_or("Home directory not found")?;
        let project = expand_home(&cwd);
        if !project.is_dir() {
            return Err("Project directory does not exist".into());
        }
        match provider.as_str() {
            "claude_desktop" => {
                if scope != "user" {
                    return Err("Unsupported scope for this provider".into());
                }
                if server.get("command").is_none() {
                    return Err("Claude Desktop local configuration requires a command".into());
                }
                write_json_server(&claude_desktop_config(Path::new(&home)), &name, server)
            }
            "claude" | "codex" => add_mcp_via_cli(
                &provider,
                &scope,
                &cwd,
                &name,
                &server,
                binary_path.as_deref(),
            ),
            _ => Err("Unsupported MCP provider".into()),
        }
    }
}

fn write_json_server(path: &Path, name: &str, server: Value) -> Result<(), String> {
    write_json_config(path, |root| {
        let object = root
            .as_object_mut()
            .ok_or("Existing config must be a JSON object")?;
        let servers = object
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));
        let servers = servers
            .as_object_mut()
            .ok_or("Existing mcpServers must be an object")?;
        if servers.contains_key(name) {
            return Err(format!("{name} is already configured in this file"));
        }
        servers.insert(name.to_owned(), server);
        Ok(())
    })
}

fn write_json_config(
    path: &Path,
    update: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid config path")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let lock_path = parent.join(format!(
        ".{}.lock",
        path.file_name()
            .and_then(|part| part.to_str())
            .unwrap_or("mcp")
    ));
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    let mut root: Value = if path.exists() {
        let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw)
            .or_else(|_| serde_json::from_str(&strip_jsonc(&raw)))
            .map_err(|e| format!("Existing config is invalid JSON: {e}"))?
    } else {
        serde_json::json!({})
    };
    update(&mut root)?;
    let encoded = serde_json::to_vec_pretty(&root).map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("mcp"),
        rand::random::<u128>()
    ));
    let result = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|e| e.to_string())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnection {
    pub provider: String,
    pub name: String,
    pub scope: String,
    pub config_path: String,
    pub transport: String,
    pub enabled: bool,
}

pub fn mcp_discover(cwd: String) -> Result<Vec<McpConnection>, String> {
    {
        let home = dirs_home().ok_or("Home directory not found")?;
        let project = expand_home(&cwd);
        let codex_home = std::env::var_os("CODEX_HOME").map(PathBuf::from);
        let desktop_config = claude_desktop_config(Path::new(&home));
        Ok(discover(
            Path::new(&home),
            &project,
            codex_home.as_deref(),
            &desktop_config,
        ))
    }
}

fn discover(
    home: &Path,
    project: &Path,
    codex_home_override: Option<&Path>,
    desktop_config: &Path,
) -> Vec<McpConnection> {
    let mut connections = Vec::new();
    let claude = home.join(".claude.json");
    if let Some(config) = read_json(&claude) {
        add_json_servers(
            &mut connections,
            "claude",
            "user",
            &claude,
            config.get("mcpServers"),
        );
        add_json_servers(
            &mut connections,
            "claude",
            "local",
            &claude,
            config
                .get("projects")
                .and_then(|projects| projects.get(project.to_string_lossy().as_ref()))
                .and_then(|entry| entry.get("mcpServers")),
        );
    }
    add_json_file(
        &mut connections,
        "claude_desktop",
        "user",
        desktop_config,
        "mcpServers",
    );

    let codex_home = codex_home_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".codex"));
    add_toml_file(
        &mut connections,
        "codex",
        "user",
        &codex_home.join("config.toml"),
    );

    // Project configuration is inherited from parent directories. Stop at the
    // repository boundary so an unrelated parent project is not shown.
    for directory in project.ancestors() {
        if directory == home {
            break;
        }
        add_json_file(
            &mut connections,
            "claude",
            "project",
            &directory.join(".mcp.json"),
            "mcpServers",
        );
        add_toml_file(
            &mut connections,
            "codex",
            "project",
            &directory.join(".codex/config.toml"),
        );
        if directory.join(".git").exists() {
            break;
        }
    }
    connections.sort_by(|a, b| {
        (&a.provider, &a.name, &a.scope, &a.config_path).cmp(&(
            &b.provider,
            &b.name,
            &b.scope,
            &b.config_path,
        ))
    });
    connections
}

fn add_json_file(
    connections: &mut Vec<McpConnection>,
    provider: &str,
    scope: &str,
    path: &Path,
    key: &str,
) {
    if let Some(config) = read_json(path) {
        add_json_servers(connections, provider, scope, path, config.get(key));
    }
}

fn add_json_servers(
    connections: &mut Vec<McpConnection>,
    provider: &str,
    scope: &str,
    path: &Path,
    servers: Option<&Value>,
) {
    let Some(servers) = servers.and_then(Value::as_object) else {
        return;
    };
    for (name, config) in servers {
        if !config.is_object() {
            continue;
        }
        connections.push(McpConnection {
            provider: provider.into(),
            name: name.clone(),
            scope: scope.into(),
            config_path: path.to_string_lossy().into_owned(),
            transport: transport(config).into(),
            enabled: config.get("enabled").and_then(Value::as_bool) != Some(false)
                && config.get("disabled").and_then(Value::as_bool) != Some(true),
        });
    }
}

fn add_toml_file(connections: &mut Vec<McpConnection>, provider: &str, scope: &str, path: &Path) {
    let Some(raw) = std::fs::read_to_string(path).ok() else {
        return;
    };
    let Some(config) = toml::from_str::<toml::Value>(&raw).ok() else {
        return;
    };
    let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) else {
        return;
    };
    for (name, entry) in servers {
        connections.push(McpConnection {
            provider: provider.into(),
            name: name.clone(),
            scope: scope.into(),
            config_path: path.to_string_lossy().into_owned(),
            transport: if entry.get("url").is_some() {
                "http"
            } else {
                "stdio"
            }
            .into(),
            enabled: entry.get("enabled").and_then(toml::Value::as_bool) != Some(false),
        });
    }
}

fn transport(config: &Value) -> &str {
    config
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if config.get("url").is_some() {
                "http"
            } else {
                "stdio"
            }
        })
}

fn read_json(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw)
        .or_else(|_| serde_json::from_str(&strip_jsonc(&raw)))
        .ok()
}

/// Remove JSONC comments and trailing commas without touching quoted text.
fn strip_jsonc(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut clean = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut quoted = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if quoted {
            clean.push(byte);
            if byte == b'\\' && index + 1 < bytes.len() {
                index += 1;
                clean.push(bytes[index]);
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
            clean.push(byte);
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            clean.push(b'\n');
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 1).min(bytes.len() - 1);
        } else {
            clean.push(byte);
        }
        index += 1;
    }
    let mut result = Vec::with_capacity(clean.len());
    quoted = false;
    let mut escaped = false;
    for (index, byte) in clean.iter().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else if *byte == b'"' {
            quoted = true;
        }
        if !quoted
            && *byte == b','
            && clean[index + 1..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| *b == b'}' || *b == b']')
        {
            continue;
        }
        result.push(*byte);
    }
    String::from_utf8(result).unwrap_or_default()
}

use std::time::Duration;
fn resolve_mcp_binary(provider: &str, path: Option<&str>) -> Result<PathBuf, String> {
    let kind = crate::provider::Kind::parse(provider)
        .filter(|kind| {
            matches!(
                kind,
                crate::provider::Kind::Claude | crate::provider::Kind::Codex
            )
        })
        .ok_or("Unsupported MCP provider")?;
    path.map(PathBuf::from)
        .or_else(|| crate::provider::resolve(kind))
        .ok_or_else(|| format!("{provider} CLI not installed"))
}
fn mcp_command(
    binary: PathBuf,
    args: Vec<String>,
    cwd: String,
    timeout: Duration,
) -> Result<String, String> {
    crate::cli::output(&binary, &args, &expand_home(&cwd), timeout)
}
pub(crate) fn add_mcp_via_cli(
    provider: &str,
    scope: &str,
    cwd: &str,
    name: &str,
    config: &serde_json::Value,
    binary_path: Option<&str>,
) -> Result<(), String> {
    let (binary, args) = mcp_add_args(provider, scope, name, config, binary_path)?;
    mcp_command(binary, args, cwd.to_owned(), Duration::from_secs(30))?;
    Ok(())
}

fn mcp_add_args(
    provider: &str,
    scope: &str,
    name: &str,
    config: &serde_json::Value,
    binary_path: Option<&str>,
) -> Result<(PathBuf, Vec<String>), String> {
    if provider == "claude" {
        if !matches!(scope, "local" | "project" | "user") {
            return Err("Invalid Claude MCP scope".into());
        }
        let binary = resolve_mcp_binary("claude", binary_path)?;
        let config = serde_json::to_string(config).map_err(|e| e.to_string())?;
        return Ok((
            binary,
            vec![
                "mcp".into(),
                "add-json".into(),
                name.into(),
                config,
                "--scope".into(),
                scope.into(),
            ],
        ));
    }
    if provider == "codex" && scope != "user" {
        return Err("Codex CLI adds user-scoped servers only".into());
    }
    let binary = match provider {
        "codex" => resolve_mcp_binary("codex", binary_path)?,
        _ => return Err("Unsupported MCP provider".into()),
    };
    let object = config
        .as_object()
        .ok_or("Server configuration must be an object")?;
    let remote = object.get("url").and_then(serde_json::Value::as_str);
    let allowed: &[&str] = if remote.is_some() {
        &["type", "url", "bearerTokenEnvVar"]
    } else {
        &["type", "command", "args", "env"]
    };
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!(
            "{provider} add cannot preserve '{key}'; edit its config file instead"
        ));
    }
    let kind = object.get("type").and_then(serde_json::Value::as_str);
    if remote.is_some() && !matches!(kind, None | Some("http") | Some("remote")) {
        return Err(format!("{provider} CLI supports HTTP URLs only"));
    }
    if remote.is_none() && !matches!(kind, None | Some("stdio") | Some("local")) {
        return Err("Local server type must be stdio".into());
    }
    let mut args = vec!["mcp".into(), "add".into(), name.into()];
    if let Some(url) = remote {
        let parsed = url::Url::parse(url).map_err(|_| "Invalid server URL")?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("MCP URL must use HTTP or HTTPS".into());
        }
        args.extend(["--url".into(), url.into()]);
        if let Some(var) = object
            .get("bearerTokenEnvVar")
            .and_then(serde_json::Value::as_str)
        {
            args.extend(["--bearer-token-env-var".into(), var.into()]);
        }
    } else {
        args.extend(mcp_key_values(config, "env", "--env")?);
        let command = object
            .get("command")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("Local server needs a command")?;
        let parameters = object
            .get("args")
            .map(|value| value.as_array().ok_or("args must be an array"))
            .transpose()?;
        args.push("--".into());
        args.push(command.into());
        for parameter in parameters.into_iter().flatten() {
            args.push(
                parameter
                    .as_str()
                    .ok_or("args must contain strings")?
                    .into(),
            );
        }
    }
    Ok((binary, args))
}

fn mcp_key_values(
    config: &serde_json::Value,
    field: &str,
    flag: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = config.get(field) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_object()
        .ok_or_else(|| format!("{field} must be an object"))?;
    let mut args = Vec::new();
    for (key, value) in values {
        let value = value
            .as_str()
            .ok_or_else(|| format!("{field} values must be strings"))?;
        args.extend([flag.to_owned(), format!("{key}={value}")]);
    }
    Ok(args)
}

pub fn login(cwd: &str, provider: &str, name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err("Invalid MCP server name".into());
    }
    let action = match provider {
        "claude" | "codex" => "login",
        _ => return Err("Unsupported MCP provider".into()),
    };
    mcp_command(
        resolve_mcp_binary(provider, None)?,
        vec!["mcp".into(), action.into(), name.into()],
        cwd.into(),
        Duration::from_secs(180),
    )
    .map(|_| ())
}
pub fn remove(cwd: &str, name: &str, scope: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid MCP server name".into());
    }
    if !matches!(scope, "local" | "project" | "user") {
        return Err("Invalid MCP scope".into());
    }
    mcp_command(
        resolve_mcp_binary("claude", None)?,
        vec![
            "mcp".into(),
            "remove".into(),
            name.into(),
            "--scope".into(),
            scope.into(),
        ],
        cwd.into(),
        Duration::from_secs(30),
    )
    .map(|_| ())
}
pub fn claude_health(cwd: &str) -> Result<String, String> {
    mcp_command(
        resolve_mcp_binary("claude", None)?,
        vec!["mcp".into(), "list".into()],
        cwd.into(),
        Duration::from_secs(30),
    )
}

#[cfg(test)]
mod tests {
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
        let root =
            std::env::temp_dir().join(format!("vorkspace-mcp-lock-{}", rand::random::<u128>()));
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

        let root =
            std::env::temp_dir().join(format!("vorkspace-mcp-mode-{}", rand::random::<u128>()));
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
        let root =
            std::env::temp_dir().join(format!("vorkspace-mcp-write-{}", rand::random::<u128>()));
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
        let root =
            std::env::temp_dir().join(format!("vorkspace-mcp-jsonc-{}", rand::random::<u128>()));
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
}
