//! File links in text and the action that opens them. Transcripts and Markdown
//! produce these, and the file editor consumes them.
use vorkspace_core::files::document;

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = file_editor, no_json)]
pub(crate) struct OpenPath(pub String);

/// File links may include a one-based line and column (`:12:4` or `#L12`).
pub(crate) fn file_location(raw: &str, cwd: &str) -> (String, Option<(usize, usize)>) {
    let mut path = raw.strip_prefix("file://").unwrap_or(raw).to_owned();
    let mut location = None;
    if let Some((file, line)) = path.rsplit_once("#L")
        && let Ok(line) = line.split('-').next().unwrap_or("").parse::<usize>()
    {
        location = Some((line.max(1), 1));
        path = file.into();
    } else if let Some((prefix, number)) = path.rsplit_once(':')
        // A range such as `:12-20` opens at its first line.
        && let Ok(last) = number.split('-').next().unwrap_or("").parse::<usize>()
    {
        if let Some((file, line)) = prefix.rsplit_once(':')
            && let Ok(line) = line.parse::<usize>()
        {
            location = Some((line.max(1), last.max(1)));
            path = file.into();
        } else {
            location = Some((last.max(1), 1));
            path = prefix.into();
        }
    }
    if raw.starts_with("file://") || raw.contains('%') {
        let bytes = path.as_bytes();
        let mut decoded = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at] == b'%'
                && at + 2 < bytes.len()
                && let Ok(hex) = std::str::from_utf8(&bytes[at + 1..at + 3])
                && let Ok(byte) = u8::from_str_radix(hex, 16)
            {
                decoded.push(byte);
                at += 3;
            } else {
                decoded.push(bytes[at]);
                at += 1;
            }
        }
        path = String::from_utf8_lossy(&decoded).into_owned();
    }
    let path = document::resolve_path(std::path::Path::new(&path), std::path::Path::new(cwd));
    (path.to_string_lossy().into_owned(), location)
}

/// A relative link is resolved against its writer's folder, but an agent
/// often means a file in another folder it works on: a Bot writes from its
/// own home about its vorkspace's projects. When the resolved file is missing,
/// look for the same relative path under the other `folders`, in their order.
/// A path that names a folder itself (`project/src/main.rs`) is tried beside it.
pub(crate) fn reroot(
    path: &str,
    folders: &[String],
    exists: impl Fn(&std::path::Path) -> bool,
) -> Option<String> {
    use std::path::Path;
    let file = Path::new(path);
    if exists(file) {
        return None;
    }
    let (base, relative) = folders
        .iter()
        .filter_map(|folder| Some((folder, file.strip_prefix(folder).ok()?)))
        .filter(|(_, relative)| !relative.as_os_str().is_empty())
        .min_by_key(|(_, relative)| relative.components().count())?;
    folders
        .iter()
        .filter(|folder| *folder != base)
        .flat_map(|folder| {
            let folder = Path::new(folder);
            let beside = folder
                .parent()
                .filter(|_| folder.file_name() == relative.iter().next())
                .map(|parent| parent.join(relative));
            [Some(folder.join(relative)), beside]
        })
        .flatten()
        .find(|candidate| exists(candidate))
        .map(|found| found.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_links_resolve_relative_paths_and_line_locations() {
        assert_eq!(
            file_location("src/../main.rs:12:4", "/project"),
            ("/project/main.rs".into(), Some((12, 4)))
        );
        assert_eq!(
            file_location("file:///project/my%20file.rs#L3", "/elsewhere"),
            ("/project/my file.rs".into(), Some((3, 1)))
        );
        assert_eq!(
            file_location("./README.md", "/project"),
            ("/project/README.md".into(), None)
        );
        assert_eq!(
            file_location("/tmp/a.rs:0", "/project"),
            ("/tmp/a.rs".into(), Some((1, 1)))
        );
        assert_eq!(
            file_location("src/a.rs:12-20", "/project"),
            ("/project/src/a.rs".into(), Some((12, 1)))
        );
    }

    #[test]
    fn a_missing_link_is_found_under_the_writers_other_folders() {
        let folders = [
            "/home/bots/atlas".to_owned(),
            "/work/app".to_owned(),
            "/work/site".to_owned(),
        ];
        let on_disk = [
            "/home/bots/atlas/MEMORY.md",
            "/work/app/src/main.rs",
            "/work/site/src/main.rs",
            "/work/site/index.html",
        ];
        let find = |path: &str| {
            reroot(path, &folders, |candidate| {
                on_disk.contains(&candidate.to_str().unwrap())
            })
        };
        // A file that exists where the link says is left alone.
        assert_eq!(find("/home/bots/atlas/MEMORY.md"), None);
        // A Bot wrote a project-relative path from its own home.
        assert_eq!(
            find("/home/bots/atlas/index.html").as_deref(),
            Some("/work/site/index.html")
        );
        // Two projects have it: the earlier folder wins.
        assert_eq!(
            find("/home/bots/atlas/src/main.rs").as_deref(),
            Some("/work/app/src/main.rs")
        );
        // The path began with the project's own folder name.
        assert_eq!(
            find("/home/bots/atlas/site/index.html").as_deref(),
            Some("/work/site/index.html")
        );
        // One project's session named a file of another.
        assert_eq!(
            find("/work/app/index.html").as_deref(),
            Some("/work/site/index.html")
        );
        // Found nowhere, or outside every folder: the link stays as written.
        assert_eq!(find("/home/bots/atlas/absent.rs"), None);
        assert_eq!(find("/elsewhere/index.html"), None);
    }
}
