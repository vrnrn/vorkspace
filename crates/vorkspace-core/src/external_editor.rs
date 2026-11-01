//! Opening a project folder in an installed code editor.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::scm::shell::dirs_home;
use crate::scm::shell::{apply_gui_env, expand_home, resolve_gui_binary};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalEditor {
    pub id: &'static str,
    pub name: &'static str,
}

struct EditorDefinition {
    id: &'static str,
    name: &'static str,
    commands: &'static [&'static str],
    mac_apps: &'static [&'static str],
}

const EDITORS: &[EditorDefinition] = &[
    EditorDefinition {
        id: "vscode",
        name: "Visual Studio Code",
        commands: &["code"],
        mac_apps: &["Visual Studio Code.app"],
    },
    EditorDefinition {
        id: "vscode-insiders",
        name: "Visual Studio Code Insiders",
        commands: &["code-insiders"],
        mac_apps: &["Visual Studio Code - Insiders.app"],
    },
    EditorDefinition {
        id: "vscodium",
        name: "VSCodium",
        commands: &["codium"],
        mac_apps: &["VSCodium.app"],
    },
    EditorDefinition {
        id: "cursor",
        name: "Cursor",
        commands: &["cursor"],
        mac_apps: &["Cursor.app"],
    },
    EditorDefinition {
        id: "zed",
        name: "Zed",
        commands: &["zed"],
        mac_apps: &["Zed.app", "Zed Preview.app"],
    },
    EditorDefinition {
        id: "windsurf",
        name: "Windsurf",
        commands: &["windsurf"],
        mac_apps: &["Windsurf.app"],
    },
    EditorDefinition {
        id: "sublime-text",
        name: "Sublime Text",
        commands: &["subl", "sublime_text"],
        mac_apps: &["Sublime Text.app"],
    },
];

enum EditorLauncher {
    Command(PathBuf),
    MacApp(PathBuf),
}

fn definition(id: &str) -> Option<&'static EditorDefinition> {
    EDITORS.iter().find(|editor| editor.id == id)
}

fn installed_mac_app(editor: &EditorDefinition) -> Option<PathBuf> {
    let user_applications = dirs_home().map(|home| PathBuf::from(home).join("Applications"));
    editor.mac_apps.iter().find_map(|name| {
        user_applications
            .as_ref()
            .map(|root| root.join(name))
            .filter(|path| path.is_dir())
            .or_else(|| {
                let path = Path::new("/Applications").join(name);
                path.is_dir().then_some(path)
            })
    })
}

fn resolve_editor(editor: &EditorDefinition) -> Option<EditorLauncher> {
    if let Some(path) = installed_mac_app(editor) {
        return Some(EditorLauncher::MacApp(path));
    }

    editor
        .commands
        .iter()
        .find_map(|command| resolve_gui_binary(command))
        .map(EditorLauncher::Command)
}

/// Editors found on this machine. Blocking: the first call may read the
/// login shell's PATH, so callers run it on a background executor.
pub fn installed() -> Vec<ExternalEditor> {
    EDITORS
        .iter()
        .filter(|editor| resolve_editor(editor).is_some())
        .map(|editor| ExternalEditor {
            id: editor.id,
            name: editor.name,
        })
        .collect()
}

/// Opens the folder `cwd` in the editor `editor_id`. Blocking, like [`installed`].
pub fn open(editor_id: &str, cwd: &str) -> Result<(), String> {
    let editor = definition(editor_id).ok_or_else(|| "Unknown external editor.".to_string())?;
    if cwd.starts_with("remote://") {
        return Err("Remote folders stay on their host machine.".into());
    }
    let cwd = expand_home(Path::new(cwd));
    if !cwd.is_dir() {
        return Err(format!("{} is not a folder.", cwd.display()));
    }
    let cwd = cwd
        .canonicalize()
        .map_err(|error| format!("Could not resolve {}: {error}", cwd.display()))?;
    let launcher =
        resolve_editor(editor).ok_or_else(|| format!("{} is no longer installed.", editor.name))?;

    let is_mac_app = matches!(&launcher, EditorLauncher::MacApp(_));
    let mut command = match launcher {
        EditorLauncher::MacApp(app) => {
            let mut command = Command::new("/usr/bin/open");
            command.arg("-a").arg(app).arg(&cwd);
            command
        }
        EditorLauncher::Command(program) => {
            let mut command = Command::new(program);
            command.arg(&cwd);
            command
        }
    };

    apply_gui_env(&mut command);
    if is_mac_app {
        let status = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("Could not open {}: {error}", editor.name))?;
        return if status.success() {
            Ok(())
        } else {
            Err(format!(
                "Could not open {} (launcher exited with {status}).",
                editor.name
            ))
        };
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open {}: {error}", editor.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_ids_and_names_are_unique() {
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for editor in EDITORS {
            assert!(ids.insert(editor.id), "duplicate editor id: {}", editor.id);
            assert!(
                names.insert(editor.name),
                "duplicate editor name: {}",
                editor.name
            );
            assert!(!editor.commands.is_empty());
        }
    }

    #[test]
    fn remote_and_missing_folders_are_rejected_before_launch() {
        assert_eq!(
            open("vscode", "remote://host/project").unwrap_err(),
            "Remote folders stay on their host machine."
        );
        assert!(open("vscode", "").unwrap_err().contains("not a folder"));
    }

    #[test]
    fn unknown_editor_is_rejected_before_launch() {
        let error = open("not-an-editor", ".").unwrap_err();
        assert_eq!(error, "Unknown external editor.");
    }
}
