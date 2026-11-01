//! Explorer filesystem operations, ported from src-tauri/src/fs.rs, plus the
//! pure name and path helpers from the files feature model.
pub mod document;
mod index;
pub(crate) use index::{git_output_capped, is_indexable_root};
pub use index::{list_project_files, list_project_files_sync_cancellable, ProjectFile};
mod name;
mod paths;
pub mod remote;
pub use remote::run_remote_command;

use std::collections::HashSet;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::scm::{expand_home, path_to_js};
pub use name::{
    leaf_name, validate_file_name, well_formed_file_name, NameIssue, NameIssueKind, Severity,
};
pub use paths::{
    create_parent_of, dirs_touched_by_create, dirs_touched_by_move, display_path,
    is_equal_or_inside, join_path, parent_path, path_key, rebase_path,
};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub ignored: bool,
}

/// Immediate children of `path` (project tree). Folders first, then natural name order.
pub fn list_dir(path: &Path) -> Result<Vec<DirEntry>, String> {
    if remote::is_remote(path) {
        return remote::command("list_dir", serde_json::json!({"path":path}));
    }
    let dir = expand_home(path);
    let reader = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let mut out = Vec::new();
    for ent in reader {
        let Ok(ent) = ent else { continue };
        let name = ent.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == ".DS_Store" {
            continue;
        }
        let path = ent.path();
        let is_dir = ent
            .file_type()
            .map(|t| t.is_dir() || (t.is_symlink() && path.is_dir()))
            .unwrap_or_else(|_| path.is_dir());
        out.push(DirEntry {
            ignored: false,
            name: name.to_string(),
            path: path_to_js(&path),
            is_dir,
        });
    }

    let names: Vec<&str> = out.iter().map(|e| e.name.as_str()).collect();
    let ignored = git_ignored_names(&dir, &names).unwrap_or_else(|| {
        let ignore = Ignore::load(&dir);
        names
            .iter()
            .filter(|n| ignore.matches(n))
            .map(|n| n.to_string())
            .collect()
    });
    for entry in &mut out {
        entry.ignored = entry.name == ".git" || ignored.contains(&entry.name);
    }

    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| compare_natural_names(&a.name, &b.name))
    });
    Ok(out)
}

/// Case-insensitive order where digit runs compare by numeric value.
pub fn compare_natural_names(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let a_start = i;
            let b_start = j;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let a_digits = &a[a_start..i];
            let b_digits = &b[b_start..j];
            let a_value = a_digits
                .iter()
                .position(|digit| *digit != b'0')
                .map_or(&a_digits[a_digits.len()..], |start| &a_digits[start..]);
            let b_value = b_digits
                .iter()
                .position(|digit| *digit != b'0')
                .map_or(&b_digits[b_digits.len()..], |start| &b_digits[start..]);
            let order = a_value
                .len()
                .cmp(&b_value.len())
                .then_with(|| a_value.cmp(b_value));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let order = a[i].to_ascii_lowercase().cmp(&b[j].to_ascii_lowercase());
            if order != Ordering::Equal {
                return order;
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| a.cmp(b))
}

const CHECK_IGNORE_SOME_MATCHED: i32 = 0;
const CHECK_IGNORE_NONE_MATCHED: i32 = 1;

fn git_ignored_names(dir: &Path, names: &[&str]) -> Option<HashSet<String>> {
    if names.is_empty() {
        return Some(HashSet::new());
    }
    let mut child = crate::scm::git_cmd()
        .arg("-C")
        .arg(dir)
        .args(["check-ignore", "--stdin", "-z"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut input = Vec::with_capacity(names.iter().map(|n| n.len() + 1).sum());
    for name in names {
        input.extend_from_slice(name.as_bytes());
        input.push(0);
    }
    let mut stdin = child.stdin.take()?;
    // Write on a separate thread: a large listing can fill the stdout pipe
    // while git still waits for stdin, which would deadlock a serial writer.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output().ok()?;
    writer.join().ok()?.ok()?;

    if !matches!(
        output.status.code(),
        Some(CHECK_IGNORE_SOME_MATCHED | CHECK_IGNORE_NONE_MATCHED)
    ) {
        return None;
    }
    Some(
        output
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect(),
    )
}

/// Name-only `.gitignore` subset for directories outside a git repository.
/// Inside a repository `git check-ignore` is the source of truth.
struct Ignore {
    exact: HashSet<String>,
    suffixes: Vec<String>,
}

impl Ignore {
    fn load(from: &Path) -> Self {
        let mut exact = HashSet::from([".git".into()]);
        let mut suffixes = Vec::new();
        let root = project_root(from);
        if let Ok(text) = std::fs::read_to_string(root.join(".gitignore")) {
            for raw in text.lines() {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                    continue;
                }
                let line = line.trim_end_matches('/');
                if line.contains('/') {
                    continue;
                }
                if let Some(ext) = line.strip_prefix("*.") {
                    if !ext.is_empty() && !ext.contains('*') {
                        suffixes.push(format!(".{ext}"));
                    }
                    continue;
                }
                exact.insert(line.to_string());
            }
        }
        Self { exact, suffixes }
    }

    fn matches(&self, name: &str) -> bool {
        self.exact.contains(name) || self.suffixes.iter().any(|s| name.ends_with(s))
    }
}

fn project_root(start: &Path) -> PathBuf {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() || dir.join(".gitignore").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return start.to_path_buf(),
        }
    }
}

fn resolve_under(parent: &Path, name: &str) -> Result<PathBuf, String> {
    if name.starts_with('/') || name.starts_with('\\') {
        return Err("A file or folder name cannot start with a slash.".into());
    }

    let trimmed = name.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.chars().all(char::is_whitespace) {
        return Err("A file or folder name must be provided.".into());
    }

    let mut dest = parent.to_path_buf();
    for segment in trimmed.split(['/', '\\']) {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." || segment.len() > 255 {
            return Err(format!(
                "The name {trimmed} is not valid as a file or folder name. Please choose a different name."
            ));
        }
        dest.push(segment);
    }

    if !dest.starts_with(parent) {
        return Err("Invalid path".into());
    }
    Ok(dest)
}

fn file_label(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
        .to_string()
}

fn already_exists(label: &str) -> String {
    format!(
        "A file or folder {label} already exists at this location. Please choose a different name."
    )
}

fn missing(path: &Path) -> String {
    format!("{}: No such file or directory", path.display())
}

/// Create a file or folder under `parent`. `name` may contain `/` or `\` to
/// nest. Returns the created path.
pub fn create_path(parent: &Path, name: &str, is_dir: bool) -> Result<String, String> {
    if remote::is_remote(parent) {
        return remote::command(
            "create_path",
            serde_json::json!({"parent":parent,"name":name,"isDir":is_dir}),
        );
    }
    let parent_dir = expand_home(parent);
    let dest = resolve_under(&parent_dir, name)?;
    let label = file_label(&dest, name);

    if dest.exists() {
        return Err(already_exists(&label));
    }

    if is_dir {
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    } else {
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::File::create_new(&dest).map_err(|e| {
            if e.kind() == ErrorKind::AlreadyExists {
                already_exists(&label)
            } else {
                e.to_string()
            }
        })?;
    }

    Ok(dest.to_string_lossy().into_owned())
}

/// Rename `path` to `name` (relative to the current parent; `/` nests).
pub fn rename_path(path: &Path, name: &str) -> Result<String, String> {
    if remote::is_remote(path) {
        return remote::command("rename_path", serde_json::json!({"path":path,"name":name}));
    }
    let from = expand_home(path);
    if !from.exists() {
        return Err(missing(&from));
    }
    let parent = from
        .parent()
        .ok_or_else(|| "File has no parent directory.".to_string())?;
    let dest = resolve_under(parent, name)?;
    if same_entry(&from, &dest) {
        if from == dest {
            return Ok(from.to_string_lossy().into_owned());
        }
        // Case-only rename on a case-insensitive filesystem.
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = parent.join(format!(
            ".{}.vorkspace-rename-{stamp}",
            file_label(&from, "tmp")
        ));
        std::fs::rename(&from, &tmp).map_err(|e| e.to_string())?;
        if let Err(e) = std::fs::rename(&tmp, &dest) {
            let _ = std::fs::rename(&tmp, &from);
            return Err(e.to_string());
        }
        return Ok(dest.to_string_lossy().into_owned());
    }
    if dest.exists() {
        return Err(already_exists(&file_label(&dest, name)));
    }
    if dest.starts_with(&from) {
        return Err("Cannot move a folder into itself.".into());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&from, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Permanently delete a file or folder (not the trash).
pub fn delete_path(path: &Path) -> Result<(), String> {
    if remote::is_remote(path) {
        return remote::command("delete_path", serde_json::json!({"path":path}));
    }
    let path = expand_home(path);
    if !path.exists() {
        return Err(missing(&path));
    }
    if path.is_dir() {
        std::fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))
    } else {
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Copy `from` into `dest_parent`, naming collisions `name copy.ext`,
/// `name copy 2.ext`, …. Returns the new path.
pub fn copy_path(from: &Path, dest_parent: &Path) -> Result<String, String> {
    if remote::is_remote(from) || remote::is_remote(dest_parent) {
        return remote::command(
            "copy_path",
            serde_json::json!({"from":from,"destParent":dest_parent}),
        );
    }
    let from = expand_home(from);
    if !from.exists() {
        return Err(missing(&from));
    }
    let dest_parent = expand_home(dest_parent);
    if !dest_parent.is_dir() {
        return Err(format!("{} is not a folder", dest_parent.display()));
    }
    if from.is_dir() && dir_contains(&from, &dest_parent) {
        return Err("Cannot paste a folder into itself.".into());
    }
    let name = unique_name_in(
        &dest_parent,
        &file_label(&from, from.to_str().unwrap_or("copy")),
    );
    let dest = dest_parent.join(&name);
    copy_recursive(&from, &dest)?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Move `from` into `dest_parent`, keeping its name. Returns the new path.
pub fn move_path(from: &Path, dest_parent: &Path) -> Result<String, String> {
    if remote::is_remote(from) || remote::is_remote(dest_parent) {
        return remote::command(
            "move_path",
            serde_json::json!({"from":from,"destParent":dest_parent}),
        );
    }
    let from = expand_home(from);
    if !from.exists() {
        return Err(missing(&from));
    }
    let dest_parent = expand_home(dest_parent);
    if !dest_parent.is_dir() {
        return Err(format!("{} is not a folder", dest_parent.display()));
    }
    if from.is_dir() && dir_contains(&from, &dest_parent) {
        return Err("Cannot paste a folder into itself.".into());
    }
    let name = file_label(&from, from.to_str().unwrap_or("item"));
    let dest = dest_parent.join(&name);
    if same_entry(&from, &dest) {
        return Ok(from.to_string_lossy().into_owned());
    }
    if dest.exists() {
        return Err(already_exists(&name));
    }
    std::fs::rename(&from, &dest).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Open a local file with its registered application. Run off the UI thread.
pub fn open_path_with_default_app(path: &Path) -> Result<(), String> {
    if remote::is_remote(path) {
        return Err("This isn’t available for projects on another machine yet.".into());
    }
    let path = expand_home(path);
    if !path.exists() {
        return Err(missing(&path));
    }
    let mut command = Command::new("open");
    command.arg(&path);
    crate::process::configure_child(&mut command);
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if reason.is_empty() {
            "No application could open this file.".into()
        } else {
            reason
        })
    }
}

/// Show `path` in Finder / Explorer, or open its folder elsewhere.
/// Blocking on Linux, where `xdg-open` may wait for the file manager.
pub fn reveal_path(path: &Path) -> Result<(), String> {
    if remote::is_remote(path) {
        return Err("This isn’t available for projects on another machine yet.".into());
    }
    let path = expand_home(path);
    if !path.exists() {
        return Err(missing(&path));
    }
    let path_str = path.to_str().ok_or_else(|| "Invalid path".to_string())?;
    let mut command = Command::new("open");
    command.args(["-R", path_str]);
    crate::process::configure_child(&mut command);
    let status = command.status().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("Could not reveal in Finder.".into());
    }
    Ok(())
}

fn same_entry(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    let Ok(a_meta) = std::fs::metadata(a) else {
        return false;
    };
    let Ok(b_meta) = std::fs::metadata(b) else {
        return false;
    };
    use std::os::unix::fs::MetadataExt;
    a_meta.dev() == b_meta.dev() && a_meta.ino() == b_meta.ino()
}

fn split_stem_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn unique_name_in(dir: &Path, name: &str) -> String {
    let (stem, ext) = split_stem_ext(name);
    let mut n = 0u32;
    loop {
        let candidate = match n {
            0 => name.to_string(),
            1 => format!("{stem} copy{ext}"),
            _ => format!("{stem} copy {n}{ext}"),
        };
        if !dir.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
        if n > 1000 {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            return format!("{stem} copy {stamp}{ext}");
        }
    }
}

fn copy_recursive(from: &Path, to: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(from).map_err(|e| format!("{}: {e}", from.display()))?;
    if meta.is_dir() {
        std::fs::create_dir(to).map_err(|e| format!("{}: {e}", to.display()))?;
        for ent in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
            let ent = ent.map_err(|e| e.to_string())?;
            copy_recursive(&ent.path(), &to.join(ent.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("{}: {e}", to.display()))
    }
}

fn dir_contains(dir: &Path, dest_parent: &Path) -> bool {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let dest_parent =
        std::fs::canonicalize(dest_parent).unwrap_or_else(|_| dest_parent.to_path_buf());
    dest_parent.starts_with(&dir)
}

#[cfg(test)]
mod tests;
