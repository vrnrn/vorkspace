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
        && let Ok(last) = number.parse::<usize>()
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
    }
}
