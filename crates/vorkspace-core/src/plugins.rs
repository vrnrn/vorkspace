//! Installed binary identities and their explicit host capabilities.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use vorkspace_plugin::{
    recording::{atomic_json, read_json},
    Context, Manifest, VERSION,
};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plugin {
    pub manifest: Manifest,
    pub executable: PathBuf,
    pub enabled: bool,
    pub bundled: bool,
    #[serde(default)]
    pub digest: Option<String>,
}
pub fn builtin(id: &str) -> Option<Manifest> {
    let (name, ui, connector, capabilities) = match id {
        "github" => ("GitHub", false, true, vec![]),
        "gitlab" => ("GitLab", false, true, vec![]),
        "jira" => ("Jira", false, true, vec![]),
        "meetings" => (
            "Meetings",
            true,
            false,
            vec!["audio.transcribe".into(), "ai.text".into()],
        ),
        _ => return None,
    };
    Some(Manifest {
        protocol_version: VERSION,
        id: id.into(),
        name: name.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        capabilities,
        ui,
        connector,
    })
}
fn configuration(root: &Path, id: &str) -> Result<PathBuf, String> {
    if !vorkspace_plugin::valid_id(id) {
        return Err("Invalid plugin id".into());
    }
    Ok(vorkspace_plugin::private_directory(root, id)?
        .parent()
        .unwrap()
        .join("plugin.json"))
}
fn checksum(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn list(root: &Path) -> Vec<Plugin> {
    let mut plugins = Vec::new();
    for id in ["github", "gitlab", "jira", "meetings"] {
        let enabled = read_json::<bool>(
            &root.join("plugins").join(format!("{id}.enabled.json")),
            1024,
        )
        .unwrap_or(true);
        plugins.push(Plugin {
            manifest: builtin(id).unwrap(),
            executable: vorkspace_plugin::builtin_executable(id).unwrap_or_default(),
            enabled,
            bundled: true,
            digest: None,
        });
    }
    if let Ok(entries) = std::fs::read_dir(root.join("plugins")) {
        for entry in entries.flatten() {
            let Ok(plugin) = read_json::<Plugin>(&entry.path().join("plugin.json"), 16 * 1024)
            else {
                continue;
            };
            if plugin.manifest.validate().is_ok()
                && builtin(&plugin.manifest.id).is_none()
                && configuration(root, &plugin.manifest.id).ok().as_ref()
                    == Some(&entry.path().join("plugin.json"))
            {
                plugins.push(plugin)
            }
        }
    }
    plugins
}
pub fn find(root: &Path, id: &str) -> Result<Plugin, String> {
    let plugin = list(root)
        .into_iter()
        .find(|p| p.manifest.id == id)
        .ok_or("Plugin is not installed")?;
    if !plugin.enabled {
        return Err("Plugin is disabled".into());
    }
    if !plugin.executable.is_file() {
        return Err(format!(
            "{} plugin is missing. Rebuild or reinstall it.",
            plugin.manifest.name
        ));
    }
    if !plugin.bundled {
        let expected = root.join("plugins").join(id).join("plugin");
        if plugin.executable != expected
            || std::fs::symlink_metadata(&expected)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
            || plugin.digest.as_deref() != Some(&checksum(&expected)?)
        {
            return Err("Plugin binary changed. Reinstall it to approve the new version.".into());
        }
    }
    Ok(plugin)
}
pub fn install(root: &Path, source: &Path) -> Result<Plugin, String> {
    // Discover and install the exact same private copy. A selected path can be
    // replaced while discovery runs; that must never approve different bytes.
    const MAX_BINARY: u64 = 256 * 1024 * 1024;
    let metadata = std::fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.len() > MAX_BINARY
    {
        return Err("Choose a regular executable no larger than 256 MiB".into());
    }
    let staging = tempfile::tempdir().map_err(|e| e.to_string())?;
    let staged = staging.path().join("plugin");
    let mut input = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)
        .map_err(|e| e.to_string())?;
    if !input.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Plugin is not a regular file".into());
    }
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&staged)
        .map_err(|e| e.to_string())?;
    if std::io::copy(&mut input.by_ref().take(MAX_BINARY + 1), &mut output)
        .map_err(|e| e.to_string())?
        > MAX_BINARY
    {
        return Err("Plugin exceeds 256 MiB".into());
    }
    output.sync_all().map_err(|e| e.to_string())?;
    drop(output);
    let context = Context {
        data_dir: staging.path().into(),
        cwd: staging.path().into(),
    };
    let (manifest, _) = vorkspace_plugin::call(
        &staged,
        None,
        &context,
        "plugin.describe",
        serde_json::Value::Null,
        Duration::from_secs(5),
        |_, _| Err("Capabilities are unavailable during installation".into()),
    )?;
    if manifest.connector {
        return Err("Custom integrations must provide their own interface (ui: true); the native Inbox connector contract is reserved for bundled connectors.".into());
    }
    if !manifest.ui {
        return Err("A custom plugin must provide an interface (ui: true)".into());
    }
    if builtin(&manifest.id).is_some() {
        return Err("A third-party plugin cannot replace a bundled plugin".into());
    }
    let path = configuration(root, &manifest.id)?;
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".binary-")
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    let mut input = std::fs::File::open(&staged).map_err(|e| e.to_string())?;
    std::io::copy(&mut input, temporary.as_file_mut()).map_err(|e| e.to_string())?;
    temporary
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    let executable = parent.join("plugin");
    temporary.persist(&executable).map_err(|e| e.to_string())?;
    let plugin = Plugin {
        manifest,
        executable: executable.clone(),
        enabled: true,
        bundled: false,
        digest: Some(checksum(&executable)?),
    };
    atomic_json(&path, &plugin)?;
    Ok(plugin)
}
pub fn set_enabled(root: &Path, id: &str, enabled: bool) -> Result<(), String> {
    if builtin(id).is_some() {
        return atomic_json(
            &root.join("plugins").join(format!("{id}.enabled.json")),
            &enabled,
        );
    }
    let path = configuration(root, id)?;
    let mut plugin: Plugin = read_json(&path, 16 * 1024)?;
    plugin.enabled = enabled;
    atomic_json(&path, &plugin)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn executable(directory: &Path, id: &str) -> PathBuf {
        let path = directory.join("candidate");
        let manifest = Manifest {
            id: id.into(),
            name: "Example".into(),
            version: "1".into(),
            protocol_version: VERSION,
            capabilities: vec![],
            ui: true,
            connector: false,
        };
        let source = format!(
            r#"#!/usr/bin/env python3
import sys,json
manifest=json.loads({:?})
for line in sys.stdin:
    request=json.loads(line)
    print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":manifest}}),flush=True)
"#,
            serde_json::to_string(&manifest).unwrap()
        );
        std::fs::write(&path, source).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    #[test]
    fn installation_pins_bytes_and_capabilities_then_disable_preserves_private_data() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("app");
        let source = executable(directory.path(), "example.widget");
        let installed = install(&root, &source).unwrap();
        assert!(installed.manifest.ui && installed.manifest.capabilities.is_empty());
        assert_eq!(
            find(&root, "example.widget").unwrap().digest,
            installed.digest
        );
        let private = vorkspace_plugin::private_directory(&root, "example.widget").unwrap();
        std::fs::create_dir_all(&private).unwrap();
        std::fs::write(private.join("draft"), "keep me").unwrap();
        set_enabled(&root, "example.widget", false).unwrap();
        assert!(find(&root, "example.widget").is_err());
        set_enabled(&root, "example.widget", true).unwrap();
        assert_eq!(
            std::fs::read_to_string(private.join("draft")).unwrap(),
            "keep me"
        );
        std::fs::write(&installed.executable, b"changed").unwrap();
        assert!(find(&root, "example.widget")
            .unwrap_err()
            .contains("Reinstall"));
        install(&root, &source).unwrap();
        assert!(find(&root, "example.widget").is_ok());
    }
    #[test]
    fn installation_rejects_reserved_identity_nonfiles_and_symlinked_storage() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("app");
        assert!(install(&root, directory.path()).is_err());
        let source = executable(directory.path(), "meetings");
        assert!(install(&root, &source).unwrap_err().contains("bundled"));
        let source = executable(directory.path(), "example.widget");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(directory.path(), root.join("plugins")).unwrap();
        assert!(install(&root, &source)
            .unwrap_err()
            .contains("real directories"));
    }
}
