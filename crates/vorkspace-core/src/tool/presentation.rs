//! Visual-only tool captions shared by restored desktop blocks and live tools.
use super::{ToolCall, ToolState};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolPresentation {
    pub action: String,
    pub target: String,
    /// Original path, before shortening it against the working directory.
    pub file_path: Option<String>,
    pub start_line: Option<u64>,
    /// Readable script, even when the caption describes a Read/Find/List.
    pub command: Option<String>,
    /// The model's own one-line explanation of a shell command, when it gave one.
    pub description: Option<String>,
    /// Top-level `&&`/`||`/`;` steps of an opaque chained command. Empty unless
    /// the command really chains two or more steps.
    pub steps: Vec<CommandStep>,
}

/// One step of a chained shell command, captioned like a standalone command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandStep {
    /// Operator joining this step to the previous one; `None` for the first.
    pub op: Option<&'static str>,
    pub command: String,
    pub action: &'static str,
    pub target: String,
    pub file_path: Option<String>,
    pub start_line: Option<u64>,
}

impl ToolPresentation {
    /// Only separate simple regex alternatives; preserve literal and complex queries.
    pub fn search_patterns(&self) -> Vec<&str> {
        let query = self.target.as_str();
        let single = || vec![query];
        if self.action != "Find" || query.contains(['\\', '[', ']', '(', ')']) {
            return single();
        }
        if let Some(command) = &self.command {
            let Some(args) = shlex::split(command) else {
                return single();
            };
            let binary = args.first().and_then(|arg| arg.rsplit('/').next());
            let extended = matches!(binary, Some("rg" | "ag" | "ack" | "egrep"))
                || (binary == Some("grep")
                    && args.iter().any(|arg| {
                        arg == "--extended-regexp"
                            || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('E'))
                    }));
            let literal_or_basic = args.iter().any(|arg| {
                matches!(arg.as_str(), "--fixed-strings" | "--basic-regexp")
                    || (arg.starts_with('-')
                        && !arg.starts_with("--")
                        && (arg.contains('F') || arg.contains('G')))
            });
            if !extended || literal_or_basic {
                return single();
            }
        }
        let patterns: Vec<_> = query.split('|').collect();
        if patterns.iter().any(|pattern| pattern.is_empty()) {
            single()
        } else {
            patterns
        }
    }
}

pub(super) fn present(call: &ToolCall, fallback_cwd: Option<&str>) -> ToolPresentation {
    if call.is_compaction() {
        return ToolPresentation {
            action: match call.state {
                ToolState::Running => "Compacting context…",
                ToolState::Completed => "Context compacted",
                ToolState::Failed => "Context compaction failed",
                ToolState::Cancelled => "Context compaction cancelled",
            }
            .into(),
            target: String::new(),
            file_path: None,
            start_line: None,
            command: None,
            description: None,
            steps: Vec::new(),
        };
    }
    let mut input = serde_json::from_str::<Value>(&call.input).unwrap_or(Value::Null);
    normalize_inputs(&mut input, 0);
    let desktop = call.display.as_ref().unwrap_or(&input["__desktop"]);
    let preview = &desktop["preview"];
    let mut records = Vec::new();
    inputs(&input, 0, &mut records);
    inputs(desktop, 0, &mut records);
    let field = |keys: &[&str]| -> Option<String> { records.iter().find_map(|v| string(v, keys)) };
    let cwd = field(&["cwd", "workingDirectory", "working_directory"])
        .or_else(|| fallback_cwd.map(str::to_owned));
    let path = field(&[
        "file_path",
        "path",
        "filePath",
        "targetFile",
        "target_file",
        "relative_workspace_path",
        "relativeWorkspacePath",
        "absolutePath",
        "uri",
        "file",
    ])
    .or_else(|| string(preview, &["path", "fileName"]))
    .or_else(|| location(desktop).and_then(|v| string(v, &["path", "filePath", "uri", "file"])));
    let query = field(&[
        "pattern",
        "query",
        "glob",
        "glob_pattern",
        "search_term",
        "regex",
    ])
    .or_else(|| string(preview, &["query"]));
    let start_line = records
        .iter()
        .find_map(|v| number(v, &["line", "offset", "startLine"]))
        .or_else(|| number(preview, &["startLine"]))
        .or_else(|| location(desktop).and_then(|v| number(v, &["line"])));
    let title = string(desktop, &["title"]).unwrap_or_else(|| call.name.clone());
    let kind = string(desktop, &["kind"])
        .filter(|kind| kind != "other")
        .or_else(|| string(preview, &["kind"]))
        .unwrap_or_default();
    let name = call.name.to_lowercase();
    let execute = matches!(
        name.as_str(),
        "bash" | "commandexecution" | "shell" | "execute" | "exec_command"
    ) || matches!(kind.as_str(), "execute" | "shell" | "bash");
    let raw_command = records
        .iter()
        .find_map(|v| {
            ["command", "cmd", "script"]
                .iter()
                .find_map(|key| command_value(&v[*key]))
        })
        .or_else(|| string(&input, &["__command"]))
        .or_else(|| execute.then(|| string(preview, &["title"])).flatten())
        .or_else(|| (execute && !input.is_object()).then(|| call.input.clone()));
    let command = raw_command
        .as_deref()
        .map(shell_command)
        .filter(|s| !s.is_empty());
    let mut shown = ToolPresentation {
        action: match name.as_str() {
            "read" => "Read",
            "write" => "Write",
            "edit" | "filechange" => "Edit",
            "grep" => "Find",
            "glob" => "List",
            "websearch" => "Search",
            _ => "",
        }
        .into(),
        target: String::new(),
        file_path: path.clone(),
        start_line,
        command,
        description: None,
        steps: Vec::new(),
    };
    if execute {
        shown.description = field(&["description"]).map(|d| d.lines().next().unwrap_or("").into());
        // Trust the provider's parsed stages; skip unknown pipeline filters.
        for key in ["commandActions", "parsed_cmd", "parsedCmd"] {
            for action in records
                .iter()
                .filter_map(|v| v[key].as_array())
                .flatten()
                .rev()
            {
                let path = string(action, &["path"]);
                let verb = match action["type"].as_str() {
                    Some("read") if path.is_some() => "Read",
                    Some("listFiles" | "list_files") if path.is_some() => "List",
                    Some("search") if string(action, &["query"]).is_some() => "Find",
                    _ => continue,
                };
                shown.action = verb.into();
                shown.target = if verb == "Find" {
                    string(action, &["query"]).unwrap()
                } else {
                    display_path(path.as_deref().unwrap(), cwd.as_deref())
                };
                shown.file_path = path;
                return shown;
            }
        }
        if let Some((action, target)) = readable_title(&title) {
            shown.action = action.into();
            shown.target = if action == "Find" {
                query.unwrap_or_else(|| target.into())
            } else {
                display_path(path.as_deref().unwrap_or(target), cwd.as_deref())
            };
            if action != "Find" && shown.file_path.is_none() && looks_path(target) {
                shown.file_path = Some(target.into());
            }
            return shown;
        }
        if let Some(intent) = shown.command.as_deref().and_then(shell_intent) {
            shown.action = intent.action.into();
            shown.target = if intent.action == "Find" {
                intent.target
            } else {
                display_path(&intent.target, cwd.as_deref())
            };
            shown.file_path = intent.path;
            shown.start_line = shown.start_line.or(intent.line);
            return shown;
        }
        shown.action = "Run".into();
        shown.steps = shown
            .command
            .as_deref()
            .map(|command| command_steps(command, cwd.as_deref()))
            .unwrap_or_default();
        // Raw execution rows expose the actual command, including shell quoting
        // and newlines. The normalized script above is only for semantic intent
        // and chain summaries; it must not shorten the readable command row.
        shown.target = raw_command
            .filter(|command| !command.trim().is_empty())
            .unwrap_or(title);
        // An execution working directory is never a file target.
        shown.file_path = None;
        return shown;
    }
    if let Some((action, target)) = readable_title(&title) {
        shown.action = action.into();
        shown.target = target.into();
    }
    if shown.action.is_empty() {
        shown.action = match kind.as_str() {
            "read" => "Read",
            "write" => "Write",
            "edit" => "Edit",
            "search" => "Find",
            _ => &call.name,
        }
        .into();
    }
    if matches!(shown.action.as_str(), "Read" | "Write" | "Edit" | "List") {
        shown.target = display_path(path.as_deref().unwrap_or(&shown.target), cwd.as_deref());
        if shown.file_path.is_none() && looks_path(&shown.target) {
            shown.file_path = Some(shown.target.clone());
        }
    } else if matches!(shown.action.as_str(), "Find" | "Search") {
        shown.target = query.clone().unwrap_or_else(|| shown.target.clone());
        if shown.target.is_empty() && !input.is_object() {
            shown.target = call.input.clone();
        }
    }
    if shown.target.is_empty() {
        shown.target = path.or(query).unwrap_or_else(|| {
            if title != call.name {
                title
            } else if input.is_object() {
                String::new()
            } else {
                call.input.clone()
            }
        });
    }
    shown.target = shown.target.lines().next().unwrap_or("").into();
    shown
}
fn location(value: &Value) -> Option<&Value> {
    let locations = value.get("locations").or_else(|| value.get("location"))?;
    locations
        .as_array()
        .and_then(|items| items.iter().find(|item| item.is_object()))
        .or_else(|| locations.is_object().then_some(locations))
}
// Parse nested argument bags on the disposable display copy only. Raw input
// stays byte-for-byte available through ToolCall::input_detail.
fn normalize_inputs(value: &mut Value, depth: usize) {
    if depth > 5 {
        return;
    }
    if let Some(text) = value.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if parsed.is_object() || parsed.is_array() {
                *value = parsed;
            }
        }
    }
    if let Some(items) = value.as_array_mut() {
        for item in items {
            normalize_inputs(item, depth + 1);
        }
    } else if let Some(object) = value.as_object_mut() {
        for key in [
            "arguments",
            "args",
            "params",
            "input",
            "rawInput",
            "raw_input",
            "__desktop",
        ] {
            if let Some(nested) = object.get_mut(key) {
                normalize_inputs(nested, depth + 1);
            }
        }
    }
}
fn inputs<'a>(value: &'a Value, depth: usize, out: &mut Vec<&'a Value>) {
    if depth > 5 {
        return;
    }
    if let Some(items) = value.as_array() {
        for item in items {
            inputs(item, depth + 1, out);
        }
        return;
    }
    if !value.is_object() {
        return;
    }
    out.push(value);
    for key in [
        "arguments",
        "args",
        "params",
        "input",
        "rawInput",
        "raw_input",
        "__desktop",
    ] {
        inputs(&value[key], depth + 1, out);
    }
}
fn string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value[*key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
}
fn number(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| {
            value[*key]
                .as_u64()
                .or_else(|| value[*key].as_str()?.parse().ok())
        })
        .filter(|n| *n > 0)
}
fn readable_title(title: &str) -> Option<(&'static str, &str)> {
    ["Read", "Find", "List", "Edit", "Write", "Search"]
        .into_iter()
        .find_map(|action| {
            title
                .strip_prefix(action)?
                .strip_prefix(' ')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|target| (action, target))
        })
}
fn looks_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\n', '\r'])
        && (path.starts_with('/')
            || path.contains('/')
            || path.contains('\\')
            || (!path.contains(char::is_whitespace) && path.contains('.')))
}
fn display_path(path: &str, cwd: Option<&str>) -> String {
    let path = path.strip_prefix("file://").unwrap_or(path);
    cwd.and_then(|cwd| path.strip_prefix(cwd.trim_end_matches(['/', '\\'])))
        .and_then(|rest| rest.strip_prefix('/').or_else(|| rest.strip_prefix('\\')))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(path)
        .into()
}
/// Render argv faithfully enough to preserve quoted filenames in details.
fn command_value(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.into());
    }
    let args: Vec<_> = value
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()?;
    shlex::try_join(args).ok()
}
pub(super) fn shell_command(command: &str) -> String {
    let mut current = command.trim().to_owned();
    for _ in 0..2 {
        let Some(parts) = shlex::split(&current) else {
            break;
        };
        let Some(binary) = parts.first().and_then(|s| s.rsplit(['/', '\\']).next()) else {
            break;
        };
        let unix = matches!(binary, "sh" | "bash" | "zsh" | "dash" | "ksh");
        let powershell = matches!(
            binary.to_lowercase().as_str(),
            "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
        );
        let cmd = matches!(binary.to_lowercase().as_str(), "cmd" | "cmd.exe");
        let flag = parts.iter().enumerate().skip(1).find(|(_, part)| {
            if unix {
                part.as_str() == "--command"
                    || (part.starts_with('-') && !part.starts_with("--") && part.contains('c'))
            } else if powershell {
                matches!(
                    part.to_lowercase().as_str(),
                    "-command" | "-c" | "-file" | "-f"
                )
            } else {
                cmd && part.eq_ignore_ascii_case("/c")
            }
        });
        let Some((at, flag)) = flag else { break };
        if powershell && matches!(flag.to_lowercase().as_str(), "-file" | "-f") {
            break;
        }
        let Some(script) = parts.get(at + 1) else {
            break;
        };
        current = if (powershell || cmd) && parts.len() > at + 2 {
            parts[at + 1..].join(" ")
        } else {
            script.trim().into()
        };
    }
    current
}
/// The directory a preceding `cd` moved later steps into. Relative file
/// targets belong to that directory, not to the tool's starting directory.
fn cd_target(part: &str) -> Option<String> {
    let args = shlex::split(part)?;
    match args.as_slice() {
        [cd, dir] if cd == "cd" && dir != "-" && !dir.starts_with('-') => Some(dir.clone()),
        _ => None,
    }
}
fn anchored(dir: Option<&str>, path: &str) -> String {
    match dir {
        Some(dir)
            if !path.starts_with(['/', '~', '\\'])
                && !path.contains("://")
                && !(dir.is_empty() || dir == ".") =>
        {
            format!(
                "{}/{}",
                dir.trim_end_matches('/'),
                path.trim_start_matches("./")
            )
        }
        _ => path.into(),
    }
}
/// Where later stages run. `Unknown` means a `cd` may or may not have taken
/// effect (`cd x || ...`, `cd -`, bare `cd`), so a relative file target cannot
/// be trusted and gets no chip.
enum Dir {
    Known(Option<String>),
    Unknown,
}
/// Apply one `cd`. Only `[start|&&|;] cd dir &&` definitely moves the stages that
/// follow; inside a pipeline it runs in a subshell and changes nothing.
fn cd_effect(dir: &mut Dir, requested: Option<String>, before: Option<&str>, after: Option<&str>) {
    if before == Some("|") || after == Some("|") {
        return;
    }
    let definite = matches!(before, None | Some("&&" | ";" | "\n")) && after == Some("&&");
    *dir = match (definite, requested, &*dir) {
        (true, Some(requested), Dir::Known(current)) => {
            Dir::Known(Some(if requested.starts_with(['/', '~']) {
                requested
            } else {
                anchored(current.as_deref(), &requested)
            }))
        }
        _ => Dir::Unknown,
    };
}
fn is_cd(part: &str) -> bool {
    shlex::split(part).is_some_and(|args| args.first().is_some_and(|bin| bin == "cd"))
}
/// Anchor a freshly produced intent. False when its directory is unknowable.
fn place(intent: &mut Intent, dir: &Dir) -> bool {
    match dir {
        Dir::Known(dir) => {
            reanchor(intent, dir.as_deref());
            true
        }
        Dir::Unknown => intent.action == "Find",
    }
}
fn reanchor(intent: &mut Intent, dir: Option<&str>) {
    if intent.action != "Find" {
        intent.target = anchored(dir, &intent.target);
        intent.path = intent.path.take().map(|path| anchored(dir, &path));
    }
}
struct Intent {
    action: &'static str,
    target: String,
    path: Option<String>,
    line: Option<u64>,
}
fn intent(action: &'static str, target: String, line: Option<u64>) -> Intent {
    Intent {
        action,
        path: (action != "Find").then(|| target.clone()),
        target,
        line,
    }
}
/// Split only at top-level `&&`, `||`, `;` and newlines. Pipelines stay one step,
/// and anything opaque (substitution, subshells, heredocs) keeps the raw command.
fn command_steps(command: &str, cwd: Option<&str>) -> Vec<CommandStep> {
    if command.len() > 4000 || command.contains("<<") {
        return Vec::new();
    }
    let bytes = command.as_bytes();
    let mut parts: Vec<(Option<&'static str>, &str)> = Vec::new();
    let mut op = None;
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut at = 0;
    while at < bytes.len() {
        let ch = bytes[at];
        if escaped {
            escaped = false;
        } else if ch == b'\\' && quote != Some(b'\'') {
            escaped = true;
        } else if matches!(ch, b'\'' | b'"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            }
        } else if ch == b'`' || (ch == b'$' && quote != Some(b'\'')) {
            return Vec::new();
        } else if quote.is_none() {
            if matches!(ch, b'(' | b')' | b'{' | b'}') {
                return Vec::new();
            }
            let next = bytes.get(at + 1).copied();
            let joined = match (ch, next) {
                (b'&', Some(b'&')) => Some(("&&", 2)),
                (b'|', Some(b'|')) => Some(("||", 2)),
                (b';', _) => Some((";", 1)),
                (b'\n', _) => Some((";", 1)),
                _ => None,
            };
            if let Some((sep, len)) = joined {
                let part = command[start..at].trim();
                if !part.is_empty() {
                    parts.push((op, part));
                    op = Some(sep);
                }
                at += len;
                start = at;
                continue;
            }
        }
        at += 1;
    }
    if quote.is_some() {
        return Vec::new();
    }
    let rest = command[start..].trim();
    if !rest.is_empty() {
        parts.push((op, rest));
    }
    if parts.len() < 2 {
        return Vec::new();
    }
    let mut dir = Dir::Known(None);
    let ops: Vec<_> = parts.iter().map(|(op, _)| *op).collect();
    parts
        .into_iter()
        .enumerate()
        .map(|(index, (op, part))| {
            let mut intent = shell_intent(part);
            if is_cd(part) {
                let before = (index > 0).then_some(op.unwrap_or(";"));
                let after = ops.get(index + 1).map(|op| op.unwrap_or(";"));
                cd_effect(&mut dir, cd_target(part), before, after);
            } else if intent.as_mut().is_some_and(|intent| !place(intent, &dir)) {
                intent = None;
            }
            CommandStep {
                op: (index > 0).then_some(op.unwrap_or(";")),
                command: part.into(),
                action: intent.as_ref().map_or("Run", |i| i.action),
                target: intent.as_ref().map_or_else(
                    || part.into(),
                    |i| {
                        if i.action == "Find" {
                            i.target.clone()
                        } else {
                            display_path(&i.target, cwd)
                        }
                    },
                ),
                file_path: intent.as_ref().and_then(|i| i.path.clone()),
                start_line: intent.and_then(|i| i.line),
            }
        })
        .collect()
}
/// Deliberately conservative fallback: reject substitution, redirects and opaque
/// stages. A command that happens to mention `cat` must never become a Read.
fn shell_intent(command: &str) -> Option<Intent> {
    if command.len() > 2000 {
        return None;
    }
    let mut stages: Vec<(String, &str)> = Vec::new();
    let mut pending = String::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (at, ch) in command.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            }
            continue;
        }
        if ch == '`' || (ch == '$' && quote != Some('\'')) {
            return None;
        }
        if quote.is_some() {
            continue;
        }
        if matches!(ch, '<' | '>' | '(' | ')') {
            return None;
        }
        if matches!(ch, ';' | '|' | '&' | '\n') {
            if !command[start..at].trim().is_empty() {
                stages.push((std::mem::take(&mut pending), command[start..at].trim()));
            }
            pending.push(ch);
            start = at + ch.len_utf8();
        }
    }
    if quote.is_some() {
        return None;
    }
    if !command[start..].trim().is_empty() {
        stages.push((std::mem::take(&mut pending), command[start..].trim()));
    }
    let mut last = None;
    let mut dir = Dir::Known(None);
    for (index, (before, stage)) in stages.iter().enumerate() {
        let before = (index > 0).then_some(before.as_str());
        let after = stages.get(index + 1).map(|(op, _)| op.as_str());
        if is_cd(stage) {
            cd_effect(&mut dir, cd_target(stage), before, after);
        }
        let args = shlex::split(stage)?;
        let bin = args.first()?.rsplit('/').next()?;
        let value_flags: &[&str] = match bin {
            "head" | "tail" => &["-n", "-c", "--lines", "--bytes"],
            "rg" | "grep" | "ag" | "ack" | "egrep" | "fgrep" => &[
                "-g",
                "--glob",
                "-t",
                "--type",
                "-A",
                "-B",
                "-C",
                "-m",
                "--max-count",
                "--context",
            ],
            "bat" | "batcat" => &["--style", "--theme", "--language", "-l"],
            _ => &[],
        };
        let mut positional = Vec::new();
        let mut skip = false;
        let mut literal = false;
        for arg in args.iter().skip(1) {
            if skip {
                skip = false;
                continue;
            }
            if arg == "--" {
                literal = true;
                continue;
            }
            if !literal && value_flags.contains(&arg.as_str()) {
                skip = true;
                continue;
            }
            if literal || !arg.starts_with('-') {
                positional.push(arg.clone());
            }
        }
        let mut fresh = true;
        last = match bin {
            "cd" | "pwd" | "echo" | "printf" | "true" | "wc" => {
                // A retained intent keeps the directory its read ran in.
                fresh = false;
                last
            }
            "cat" | "bat" | "batcat" | "nl" | "less" | "more" | "tac" | "head" | "tail" => {
                // Multiple files need their real command, not a misleading single-file caption.
                if positional.len() != 1 {
                    return None;
                }
                Some(intent("Read", positional[0].clone(), None))
            }
            "sed" => {
                // Only a print-address script is known to be a read. Never sed -i.
                if args
                    .iter()
                    .any(|a| a == "-i" || a.starts_with("-i") || a.starts_with("--in-place"))
                    || positional.len() != 2
                {
                    return None;
                }
                let script = positional[0].strip_suffix('p')?;
                if !script
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, ',' | '$'))
                {
                    return None;
                }
                let line = script.split(',').next()?.parse().ok();
                Some(intent("Read", positional[1].clone(), line))
            }
            "rg" if args.iter().any(|a| a == "--files") => {
                Some(intent("Find", "files".into(), None))
            }
            "rg" | "grep" | "egrep" | "fgrep" | "ag" | "ack" => {
                // Pattern files and replacement/preprocessor modes aren't simple searches.
                if args
                    .iter()
                    .any(|a| matches!(a.as_str(), "-f" | "--file" | "--pre" | "--replace" | "-r"))
                {
                    return None;
                }
                let query = args
                    .windows(2)
                    .find(|pair| matches!(pair[0].as_str(), "-e" | "--regexp"))
                    .map(|pair| pair[1].clone())
                    .or_else(|| positional.first().cloned())?;
                Some(intent("Find", query, None))
            }
            "ls" | "tree" if positional.len() == 1 => {
                Some(intent("List", positional[0].clone(), None))
            }
            _ => return None,
        };
        if fresh && last.as_mut().is_some_and(|intent| !place(intent, &dir)) {
            return None;
        }
    }
    last
}

#[cfg(test)]
mod tests;
