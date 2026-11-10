//! Project content search ported from the Tauri search service.
use crate::files::list_project_files_sync_cancellable;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Component, Path},
    sync::atomic::{AtomicBool, Ordering},
};
const MAX_MATCHES: usize = 500;
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_GIT_GREP_BYTES: usize = 4 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    pub cwd: String,
    pub query: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub regex: bool,
    pub include: Option<String>,
    pub exclude: Option<String>,
    #[serde(default)]
    pub search_id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub path: String,
    pub relative: String,
    pub line: u32,
    pub column: u32,
    pub preview: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
}

/// Byte ranges for the first highlighted match in each project-search preview.
/// Compile once for the complete result set, away from the UI's paint path.
pub fn preview_ranges(
    matches: &[SearchMatch],
    options: &SearchOptions,
) -> Vec<Option<std::ops::Range<usize>>> {
    let matcher = matcher(options, options.query.trim()).ok();
    matches
        .iter()
        .map(|m| {
            first_match(m.preview.trim_end(), matcher.as_ref()?, options.whole_word)
                .map(|found| found.range())
        })
        .collect()
}

pub fn search_project(
    root: &Path,
    options: &SearchOptions,
    cancel: &AtomicBool,
) -> Result<SearchResult, String> {
    if crate::files::remote::is_remote(root) {
        if options.query.trim().is_empty() || cancel.load(Ordering::Acquire) {
            return Ok(SearchResult {
                matches: Vec::new(),
                truncated: false,
            });
        }
        let mut options = serde_json::to_value(options).map_err(|error| error.to_string())?;
        options["cwd"] = serde_json::json!(root);
        let result = crate::files::remote::command(
            "search_project",
            serde_json::json!({"options":options}),
        )?;
        return Ok(if cancel.load(Ordering::Acquire) {
            SearchResult {
                matches: Vec::new(),
                truncated: false,
            }
        } else {
            result
        });
    }
    let expanded = crate::shell_env::expand_home(root);
    let root = expanded.as_path();
    let query = options.query.trim();
    if query.is_empty() || cancel.load(Ordering::Acquire) || !crate::files::is_indexable_root(root)
    {
        return Ok(SearchResult {
            matches: Vec::new(),
            truncated: false,
        });
    }

    if let Some(result) = git_grep(root, options, query, cancel) {
        return Ok(result);
    }

    scan_files(root, options, query, cancel)
}

fn git_grep(
    root: &Path,
    options: &SearchOptions,
    query: &str,
    cancel: &AtomicBool,
) -> Option<SearchResult> {
    git_grep_capped(root, options, query, MAX_GIT_GREP_BYTES, cancel)
}

fn git_grep_capped(
    root: &Path,
    options: &SearchOptions,
    query: &str,
    max_bytes: usize,
    cancel: &AtomicBool,
) -> Option<SearchResult> {
    let mut args = vec![
        "grep",
        "-I",
        "-z",
        "-n",
        "--column",
        "--no-color",
        "--no-full-name",
        "--no-textconv",
        "--untracked",
    ];
    if !options.case_sensitive {
        args.push("-i");
    }
    if options.whole_word {
        args.push("-w");
    }
    args.push(if options.regex { "-E" } else { "-F" });
    args.extend(["-e", query]);

    // Terminate option parsing so an include glob starting with `-` is treated
    // as a pathspec instead of a git grep flag.
    args.push("--");
    let pathspecs = pathspecs(options.include.as_deref(), options.exclude.as_deref());
    args.extend(pathspecs.iter().map(String::as_str));

    let (raw, mut truncated) =
        match crate::scm::git_output_capped(root, &args, max_bytes, Some(cancel)) {
            Some(output) => output,
            None if cancel.load(Ordering::Acquire) => {
                return Some(SearchResult {
                    matches: Vec::new(),
                    truncated: false,
                });
            }
            None => return None,
        };
    let mut matches = Vec::new();
    let mut records = raw.as_slice();
    while !records.is_empty() {
        if cancel.load(Ordering::Acquire) {
            return Some(SearchResult {
                matches: Vec::new(),
                truncated: false,
            });
        }
        let Some((path_bytes, next)) = take_until(records, 0) else {
            break;
        };
        let Some((line_bytes, next)) = take_until(next, 0) else {
            break;
        };
        let Some((column_bytes, next)) = take_until(next, 0) else {
            break;
        };
        let (preview, rest) = match take_until(next, b'\n') {
            Some(complete) => complete,
            None if !truncated => (next, &[][..]),
            None => break,
        };
        records = rest;
        let (Ok(relative), Some(line), Some(column)) = (
            std::str::from_utf8(path_bytes),
            std::str::from_utf8(line_bytes)
                .ok()
                .and_then(|n| n.parse::<u32>().ok()),
            std::str::from_utf8(column_bytes)
                .ok()
                .and_then(|n| n.parse::<usize>().ok()),
        ) else {
            continue;
        };
        if relative.is_empty()
            || !Path::new(relative)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            continue;
        }
        if matches.len() == MAX_MATCHES {
            truncated = true;
            break;
        }
        // Git returns a one-based byte column. The file editor navigates by
        // Unicode scalars, including when a preview is decoded lossily.
        let prefix = &preview[..column.saturating_sub(1).min(preview.len())];
        matches.push(SearchMatch {
            path: crate::shell_env::path_to_js(&root.join(relative)),
            relative: relative.to_owned(),
            line,
            column: String::from_utf8_lossy(prefix).chars().count() as u32 + 1,
            preview: String::from_utf8_lossy(preview).into_owned(),
        });
    }

    Some(SearchResult { matches, truncated })
}

fn take_until(bytes: &[u8], delimiter: u8) -> Option<(&[u8], &[u8])> {
    let end = bytes.iter().position(|byte| *byte == delimiter)?;
    Some((&bytes[..end], &bytes[end + 1..]))
}

fn scan_files(
    root: &Path,
    options: &SearchOptions,
    query: &str,
    cancel: &AtomicBool,
) -> Result<SearchResult, String> {
    if cancel.load(Ordering::Acquire) {
        return Ok(SearchResult {
            matches: Vec::new(),
            truncated: false,
        });
    }

    let files = list_project_files_sync_cancellable(&root.to_string_lossy(), Some(cancel))?;
    if cancel.load(Ordering::Acquire) {
        return Ok(SearchResult {
            matches: Vec::new(),
            truncated: false,
        });
    }
    let include = path_filter(options.include.as_deref())?;
    let exclude = path_filter(options.exclude.as_deref())?;
    let matcher = matcher(options, query).map_err(|error| error.to_string())?;
    let mut matches = Vec::new();
    let mut truncated = false;

    'files: for file in files {
        if cancel.load(Ordering::Acquire) {
            return Ok(SearchResult {
                matches: Vec::new(),
                truncated: false,
            });
        }
        if include
            .as_ref()
            .is_some_and(|set| !set.is_match(&file.relative))
            || exclude
                .as_ref()
                .is_some_and(|set| set.is_match(&file.relative))
        {
            continue;
        }
        let Some(content) = read_text(Path::new(&file.path)) else {
            continue;
        };

        for (index, line) in content.lines().enumerate() {
            if cancel.load(Ordering::Acquire) {
                return Ok(SearchResult {
                    matches: vec![],
                    truncated: false,
                });
            }
            if let Some(found) = first_match(line, &matcher, options.whole_word) {
                if matches.len() == MAX_MATCHES {
                    truncated = true;
                    break 'files;
                }
                matches.push(SearchMatch {
                    path: file.path.clone(),
                    relative: file.relative.clone(),
                    line: (index + 1) as u32,
                    column: line[..found.start()].chars().count() as u32 + 1,
                    preview: line.to_string(),
                });
            }
        }
    }

    Ok(SearchResult { matches, truncated })
}

fn read_text(path: &Path) -> Option<String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FILE_BYTES || bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn matcher(options: &SearchOptions, query: &str) -> Result<regex::Regex, regex::Error> {
    let pattern = if options.regex {
        std::borrow::Cow::Borrowed(query)
    } else {
        std::borrow::Cow::Owned(regex::escape(query))
    };
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .build()
}

fn first_match<'a>(
    line: &'a str,
    matcher: &regex::Regex,
    whole_word: bool,
) -> Option<regex::Match<'a>> {
    matcher
        .find_iter(line)
        .find(|m| !whole_word || is_word_boundary(line, m.start(), m.len()))
}

fn is_word_boundary(line: &str, start: usize, len: usize) -> bool {
    let word = |ch: char| ch.is_alphanumeric() || ch == '_';
    !line[..start].chars().next_back().is_some_and(word)
        && !line[start + len..].chars().next().is_some_and(word)
}

fn glob_tokens(value: Option<&str>) -> impl Iterator<Item = &str> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
}

fn path_filter(value: Option<&str>) -> Result<Option<regex::RegexSet>, String> {
    let mut patterns = Vec::new();
    for glob in glob_tokens(value) {
        let glob = glob.trim_start_matches("./").trim_end_matches('/');
        let mut pattern = regex::escape(glob).replace("\\*", ".*").replace("\\?", ".");
        if !glob.contains(['*', '?']) {
            pattern.push_str("(?:/.*)?");
        }
        patterns.push(format!(r"\A{pattern}\z"));
    }
    if patterns.is_empty() {
        return Ok(None);
    }
    regex::RegexSetBuilder::new(patterns)
        .dot_matches_new_line(true)
        .build()
        .map(Some)
        .map_err(|error| error.to_string())
}

fn pathspecs(include: Option<&str>, exclude: Option<&str>) -> Vec<String> {
    glob_tokens(include)
        .map(str::to_owned)
        .chain(glob_tokens(exclude).map(|glob| format!(":(exclude){glob}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(query: &str) -> SearchOptions {
        SearchOptions {
            cwd: String::new(),
            query: query.into(),
            case_sensitive: false,
            whole_word: false,
            regex: false,
            include: None,
            exclude: None,
            search_id: String::new(),
        }
    }
    fn git(root: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    #[test]
    fn fallback_finds_literal_content_and_unicode_columns_and_skips_binary() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "first\né🙂 Needle.*\n").unwrap();
        std::fs::write(dir.path().join("binary"), b"\0Needle.*").unwrap();
        let result =
            search_project(dir.path(), &options("needle.*"), &AtomicBool::new(false)).unwrap();
        assert_eq!(result.matches.len(), 1);
        assert_eq!((result.matches[0].line, result.matches[0].column), (2, 4));
        assert!(!result.truncated);
        assert!(
            search_project(dir.path(), &options("absent"), &AtomicBool::new(false))
                .unwrap()
                .matches
                .is_empty()
        );
        assert!(
            search_project(dir.path(), &options("needle"), &AtomicBool::new(true))
                .unwrap()
                .matches
                .is_empty()
        );
    }
    #[test]
    fn git_search_honors_filters_and_never_parses_partial_records_at_cap() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("many.txt"), "needle\n".repeat(2000)).unwrap();
        std::fs::write(dir.path().join("other.rs"), "needle\n").unwrap();
        git(dir.path(), &["add", "."]);
        let cancel = AtomicBool::new(false);
        let mut opts = options("needle");
        opts.include = Some("*.rs".into());
        let found = search_project(dir.path(), &opts, &cancel).unwrap();
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].relative, "other.rs");
        let capped =
            git_grep_capped(dir.path(), &options("needle"), "needle", 256, &cancel).unwrap();
        assert!(capped.truncated);
        assert!(!capped.matches.is_empty());
        assert!(capped
            .matches
            .iter()
            .all(|m| m.preview == "needle" && m.line > 0));
        let limited = search_project(dir.path(), &options("needle"), &cancel).unwrap();
        assert_eq!(limited.matches.len(), 500);
        assert!(limited.truncated);
        assert!(search_project(dir.path(), &options("nothing"), &cancel)
            .unwrap()
            .matches
            .is_empty());
    }

    #[test]
    fn git_search_preserves_literal_names_and_uses_the_regex_match_column() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        let relative = "a\\b\nnotes.txt";
        let path = dir.path().join(relative);
        std::fs::write(&path, "é🙂 target42\n").unwrap();
        std::fs::write(dir.path().join("binary"), b"\0target42").unwrap();
        let mut opts = options("target[0-9]+");
        opts.regex = true;
        let hits = search_project(dir.path(), &opts, &AtomicBool::new(false)).unwrap();
        assert_eq!(hits.matches.len(), 1);
        let first = &hits.matches[0];
        assert_eq!((first.relative.as_str(), first.column), (relative, 4));
        assert_eq!(Path::new(&first.path), path);
    }

    #[test]
    fn ordinary_folders_support_regex_and_anchored_globs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        for path in ["src/a.rs", "src/a.txt", "src/skip.rs", "outside.rs"] {
            std::fs::write(dir.path().join(path), "🙂 needle7").unwrap();
        }
        let mut opts = options("needle[0-9]+");
        opts.regex = true;
        opts.include = Some("src/*.rs".into());
        opts.exclude = Some("src/skip.?s".into());
        let hits = search_project(dir.path(), &opts, &AtomicBool::new(false)).unwrap();
        assert_eq!(hits.matches.len(), 1);
        assert_eq!(
            (hits.matches[0].relative.as_str(), hits.matches[0].column),
            ("src/a.rs", 3)
        );
        opts.query = "[".into();
        assert!(search_project(dir.path(), &opts, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn whole_word_unicode_does_not_slice_inside_utf8() {
        let mut opts = options("é");
        opts.whole_word = true;
        let line = "café é!";
        let matcher = matcher(&opts, "é").unwrap();
        let found = first_match(line, &matcher, true).unwrap();
        assert_eq!(line[..found.start()].chars().count() + 1, 6);
    }
}
