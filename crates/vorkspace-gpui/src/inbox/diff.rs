//! Provider patches use the same virtualized native review as local changes.
use crate::diff_view::DiffFile;
use gpui::SharedString;
use std::{collections::HashMap, sync::Arc};
use vorkspace_core::{
    diff::{Block, Line, LineKind},
    inbox::Diff,
};

fn patch_path(section: &str) -> Option<String> {
    let headers = section
        .lines()
        .take_while(|line| !line.starts_with("@@ "))
        .collect::<Vec<_>>();
    headers
        .iter()
        .find_map(|line| line.strip_prefix("+++ "))
        .filter(|path| *path != "/dev/null")
        .or_else(|| headers.iter().find_map(|line| line.strip_prefix("--- ")))
        .map(|path| {
            let path = serde_json::from_str::<String>(path).unwrap_or_else(|_| path.to_string());
            path.strip_prefix("b/")
                .or_else(|| path.strip_prefix("a/"))
                .unwrap_or(&path)
                .to_string()
        })
}
fn hunk_start(value: &str) -> u32 {
    value
        .trim_start_matches(['+', '-'])
        .split(',')
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}
fn blocks(section: &str) -> Vec<Block> {
    let (mut old, mut new) = (0, 0);
    let mut lines = vec![];
    let mut in_hunk = false;
    for raw in section.lines() {
        if raw.starts_with("@@ ") {
            let mut parts = raw.split_whitespace().skip(1);
            old = hunk_start(parts.next().unwrap_or("-1"));
            new = hunk_start(parts.next().unwrap_or("+1"));
            in_hunk = true;
            lines.push(Line {
                kind: LineKind::Hunk,
                text: raw.into(),
                old_number: None,
                new_number: None,
                chunk: None,
            });
            continue;
        }
        if !in_hunk {
            continue;
        }
        let (kind, old_number, new_number) = match raw.as_bytes().first() {
            Some(b'+') => {
                let n = new;
                new += 1;
                (LineKind::Add, None, Some(n))
            }
            Some(b'-') => {
                let n = old;
                old += 1;
                (LineKind::Del, Some(n), None)
            }
            Some(b' ') => {
                let (o, n) = (old, new);
                old += 1;
                new += 1;
                (LineKind::Context, Some(o), Some(n))
            }
            _ => continue,
        };
        lines.push(Line {
            kind,
            text: raw[1..].into(),
            old_number,
            new_number,
            chunk: None,
        });
    }
    if lines.is_empty() {
        vec![]
    } else {
        vec![Block::Hunk { lines, chunk: None }]
    }
}
pub(super) fn models(diff: &Diff) -> Vec<DiffFile> {
    let sections = diff
        .patch
        .strip_prefix("diff --git ")
        .unwrap_or("")
        .split("\ndiff --git ")
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>();
    let by_path: HashMap<_, _> = sections
        .iter()
        .filter_map(|section| patch_path(section).map(|path| (path, *section)))
        .collect();
    diff.files
        .iter()
        .enumerate()
        .map(|(ix, file)| {
            let section = by_path
                .get(&file.path)
                .copied()
                // Omitted API patches break positional alignment. Never show
                // another file's hunks under a missing file's header.
                .or_else(|| {
                    (!diff.truncated && sections.len() == diff.files.len()).then(|| sections[ix])
                })
                .unwrap_or("");
            let binary = section
                .lines()
                .take_while(|line| !line.starts_with("@@ "))
                .any(|line| line.starts_with("Binary files ") || line == "GIT binary patch");
            DiffFile {
                id: SharedString::from(format!("inbox-diff-{}", file.path)),
                path: file.path.clone(),
                label: file.path.clone(),
                binary,
                too_large: diff.truncated && section.is_empty(),
                empty_message: if section.is_empty() && !diff.truncated {
                    Some("No textual diff".into())
                } else {
                    None
                },
                additions: file.additions.max(0).min(u32::MAX as i64) as u32,
                deletions: file.deletions.max(0).min(u32::MAX as i64) as u32,
                blocks: Arc::new(blocks(section)),
                can_stage: false,
                can_discard: false,
                can_stage_hunk: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_patches_never_borrow_another_files_hunks() {
        let diff = Diff {
            files: ["missing.rs", "present.rs", "deleted.rs"].into_iter().map(|path| vorkspace_core::inbox::DiffFile {
                path: path.into(), additions: 1, deletions: 1,
            }).collect(),
            patch: "diff --git a/present.rs b/present.rs\n--- a/present.rs\n+++ b/present.rs\n@@ -1 +1 @@\n-old\n+present\ndiff --git a/deleted.rs b/deleted.rs\n--- a/deleted.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-deleted\n".into(),
            truncated: true,
            ..Diff::default()
        };
        let files = models(&diff);
        assert!(files[0].blocks.is_empty());
        assert!(files[0].too_large);
        assert_eq!(files[1].blocks[0].lines()[2].text, "present");
        assert_eq!(files[2].blocks[0].lines()[1].text, "deleted");
        assert!(!files[2].too_large);
    }

    #[test]
    fn quoted_rename_headers_and_source_lines_keep_their_own_paths() {
        let path = "space \"quoted\"/λ\nfile.rs";
        let section = format!(
            "\"a/old.rs\" {}\n--- \"a/old.rs\"\n+++ {}\n@@ -1 +1 @@\n-old\n+++ something\n",
            serde_json::to_string(&format!("b/{path}")).unwrap(),
            serde_json::to_string(&format!("b/{path}")).unwrap()
        );
        assert_eq!(patch_path(&section).as_deref(), Some(path));
        let deleted =
            "a/deleted b/deleted\n--- a/deleted\n+++ /dev/null\n@@ -1 +0,0 @@\n--- source line\n";
        assert_eq!(patch_path(deleted).as_deref(), Some("deleted"));
    }

    #[test]
    fn diff_metadata_inside_source_does_not_split_or_hide_a_file() {
        let diff = Diff {
            files: ["test.rs", "image.png"].into_iter().map(|path| vorkspace_core::inbox::DiffFile { path: path.into(), additions: 2, deletions: 0 }).collect(),
            patch: "diff --git a/test.rs b/test.rs\n--- /dev/null\n+++ b/test.rs\n@@ -0,0 +1,2 @@\n+let patch = \"diff --git a/other b/other\";\n+let binary = \"Binary files a/image and b/image differ\";\ndiff --git a/image.png b/image.png\nBinary files a/image.png and b/image.png differ\n".into(),
            ..Diff::default()
        };
        let files = models(&diff);
        assert!(!files[0].binary);
        assert!(files[1].binary);
        assert_eq!(files[0].blocks[0].lines().len(), 3);
        assert!(files[0].blocks[0].lines()[2].text.starts_with("let binary"));
    }
}
