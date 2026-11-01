//! The unified review's diff model: unifiedDiff.ts, lineDiff.ts and the hunk
//! staging in editorGit.ts. Files are compared line by line (each line with
//! its newline, as git and lineDiff.ts do), then grouped into chunks exactly
//! as CodeMirror's `Chunk.build` with `presentableDiff` does: changes less
//! than three UTF-16 units apart merge, and chunks cover whole lines.
use similar::{capture_diff_deadline, Algorithm, DiffOp};
use std::time::{Duration, Instant};

pub const CONTEXT_DEFAULT: usize = 3;
/// lineDiff.ts's line pass budget. Past it, Myers replaces whatever range it
/// has not finished as a whole: still a valid diff, but coarser, and a matter
/// of wall-clock timing. Chunks are therefore kept, never derived twice.
const LINE_PASS_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineKind {
    Add,
    Del,
    Context,
    Hunk,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    pub text: String,
    pub old_number: Option<u32>,
    pub new_number: Option<u32>,
    /// The change chunk an added or deleted line belongs to: what TS keeps
    /// as `pos` to stage that hunk.
    pub chunk: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Hunk {
        lines: Vec<Line>,
        chunk: Option<usize>,
    },
    Fold {
        id: String,
        lines: Vec<Line>,
    },
}
impl Block {
    pub fn lines(&self) -> &[Line] {
        match self {
            Self::Hunk { lines, .. } | Self::Fold { lines, .. } => lines,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UnifiedFile {
    pub additions: u32,
    pub deletions: u32,
    pub lines: Vec<Line>,
    pub blocks: Vec<Block>,
    /// The ranges `Line::chunk` indexes, for `stage_chunk`.
    pub chunks: Vec<Change>,
}

pub fn build_unified_file(original: &str, current: &str, context: usize) -> UnifiedFile {
    let deadline = Instant::now() + LINE_PASS_TIMEOUT;
    unified_file(original, current, context, Some(deadline))
}

fn unified_file(
    original: &str,
    current: &str,
    context: usize,
    deadline: Option<Instant>,
) -> UnifiedFile {
    let (lines, chunks) = unified_lines(original, current, deadline);
    let additions = lines.iter().filter(|l| l.kind == LineKind::Add).count() as u32;
    let deletions = lines.iter().filter(|l| l.kind == LineKind::Del).count() as u32;
    let blocks = fold_unified_lines(&lines, context);
    UnifiedFile {
        additions,
        deletions,
        lines,
        blocks,
        chunks,
    }
}

/// Context beyond `context` lines from any change folds away; the rest
/// form hunks, each carrying its first line's chunk.
pub fn fold_unified_lines(lines: &[Line], context: usize) -> Vec<Block> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut visible: Vec<bool> = lines.iter().map(|l| l.kind != LineKind::Context).collect();
    if context > 0 {
        for (index, line) in lines.iter().enumerate() {
            if matches!(line.kind, LineKind::Context | LineKind::Hunk) {
                continue;
            }
            let from = index.saturating_sub(context);
            let to = index.saturating_add(context).min(lines.len() - 1);
            for inner in from..=to {
                if lines[inner].kind == LineKind::Context {
                    visible[inner] = true;
                }
            }
        }
    }
    let mut blocks = Vec::new();
    let mut index = 0;
    let mut fold_id = 0;
    while index < lines.len() {
        let hidden = |i: usize| lines[i].kind == LineKind::Context && !visible[i];
        if hidden(index) {
            let start = index;
            while index < lines.len() && hidden(index) {
                index += 1;
            }
            blocks.push(Block::Fold {
                id: format!("fold-{fold_id}"),
                lines: lines[start..index].to_vec(),
            });
            fold_id += 1;
            continue;
        }
        let start = index;
        let mut chunk = None;
        while index < lines.len() && !hidden(index) {
            chunk = chunk.or(lines[index].chunk);
            index += 1;
        }
        blocks.push(Block::Hunk {
            lines: lines[start..index].to_vec(),
            chunk,
        });
    }
    blocks
}

/// stageChunkText: `original` with only that chunk of `current` applied. The
/// chunk is one the view showed for these texts; diffing again here could
/// miss the line pass budget and resolve its index to a different range.
pub fn stage_chunk(original: &str, current: &str, range: Change) -> String {
    let old = Doc::new(original);
    let new = Doc::new(current);
    // applySide: the chunk's lines without their last newline, plus one
    // when the replaced range ends inside the original.
    let mut insert = new.slice(range.from_b, range.from_b.max(range.to_b.saturating_sub(1)));
    if range.from_b != range.to_b && range.to_a <= old.len {
        insert.push('\n');
    }
    let from = old.byte(range.from_a);
    let to = old.byte(range.to_a.min(old.len));
    format!("{}{}{}", &original[..from], insert, &original[to..])
}

/// diffComment.ts's `diffCommentLocation`: `path:line`.
pub fn comment_location(path: &str, line: &Line) -> String {
    let number = if line.kind == LineKind::Del {
        line.old_number
    } else {
        line.new_number
    };
    match number {
        Some(n) => format!("{path}:{n}"),
        None => path.to_owned(),
    }
}

/// formatDiffComment: a quoted line and the comment, for the composer.
pub fn format_comment(path: &str, line: &Line, comment: &str) -> String {
    let body = comment.replace("\r\n", "\n").replace('\r', "\n");
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    let marker = match line.kind {
        LineKind::Add => "+",
        LineKind::Del => "-",
        _ => " ",
    };
    let deleted = if line.kind == LineKind::Del {
        " (deleted line)"
    } else {
        ""
    };
    let location = comment_location(path, line).replace('`', "\\`");
    format!(
        "Diff comment on `{location}`{deleted}:\n\n> {marker}{}\n\n{body}",
        line.text
    )
}

/// Monospace columns: tab stops of 4, wide CJK and emoji 2, marks 0.
pub fn visual_columns(text: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut columns = 0;
    for ch in text.chars() {
        columns += match ch {
            '\t' => 4 - columns % 4,
            c if (c as u32) < 128 => 1,
            c => c.width().unwrap_or(1),
        };
    }
    columns
}

/// CodeMirror's `Text` over `value.split("\n")`, addressed in UTF-16 units.
struct Doc<'a> {
    source: &'a str,
    lines: Vec<&'a str>,
    /// UTF-16 offset of each line's start.
    starts: Vec<usize>,
    /// Byte offset of each line's start.
    bytes: Vec<usize>,
    len: usize,
}
impl<'a> Doc<'a> {
    fn new(source: &'a str) -> Self {
        let lines: Vec<&str> = source.split('\n').collect();
        let mut starts = Vec::with_capacity(lines.len());
        let mut bytes = Vec::with_capacity(lines.len());
        let (mut at, mut byte) = (0, 0);
        for line in &lines {
            starts.push(at);
            bytes.push(byte);
            at += utf16_len(line) + 1;
            byte += line.len() + 1;
        }
        Self {
            source,
            len: at - 1,
            lines,
            starts,
            bytes,
        }
    }
    /// The 0-based line containing `pos`.
    fn line_at(&self, pos: usize) -> usize {
        self.starts
            .partition_point(|start| *start <= pos)
            .saturating_sub(1)
    }
    fn line_to(&self, line: usize) -> usize {
        self.starts[line] + utf16_len(self.lines[line])
    }
    /// The byte offset of a UTF-16 offset.
    fn byte(&self, pos: usize) -> usize {
        let line = self.line_at(pos);
        let mut units = self.starts[line];
        let mut byte = self.bytes[line];
        for ch in self.lines[line].chars() {
            if units >= pos {
                break;
            }
            units += ch.len_utf16();
            byte += ch.len_utf8();
        }
        if units < pos {
            // Past the line's end: its newline.
            byte += 1;
        }
        byte.min(self.source.len())
    }
    fn slice(&self, from: usize, to: usize) -> String {
        self.source[self.byte(from)..self.byte(to.min(self.len))].to_owned()
    }
    /// linesInRange: 1-based numbers and texts of the lines `from..to` touches.
    fn lines_in_range(&self, from: usize, to: usize) -> Vec<(u32, &'a str)> {
        if from >= to || self.len == 0 {
            return Vec::new();
        }
        let end = from.max(to - 1).min(self.len.saturating_sub(1));
        let first = self.line_at(from.min(self.len));
        let last = self.line_at(end);
        (first..=last)
            .map(|i| (i as u32 + 1, self.lines[i]))
            .collect()
    }
}
fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    from_a: usize,
    to_a: usize,
    from_b: usize,
    to_b: usize,
}

/// lineDiff.ts: each line with its newline is one symbol; the result maps
/// back to UTF-16 offsets.
fn line_changes(old: &Doc, new: &Doc, deadline: Option<Instant>) -> Vec<Change> {
    fn tokens(source: &str) -> Vec<&str> {
        source.split_inclusive('\n').collect()
    }
    fn offsets(tokens: &[&str]) -> Vec<usize> {
        let mut out = Vec::with_capacity(tokens.len() + 1);
        let mut at = 0;
        out.push(0);
        for token in tokens {
            at += utf16_len(token);
            out.push(at);
        }
        out
    }
    let (a, b) = (tokens(old.source), tokens(new.source));
    let (oa, ob) = (offsets(&a), offsets(&b));
    let ops = capture_diff_deadline(Algorithm::Myers, &a, 0..a.len(), &b, 0..b.len(), deadline);
    let mut changes: Vec<Change> = Vec::new();
    for op in ops {
        let (old_range, new_range) = match op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => (old_index..old_index + old_len, new_index..new_index),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => (old_index..old_index, new_index..new_index + new_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (
                old_index..old_index + old_len,
                new_index..new_index + new_len,
            ),
        };
        let change = Change {
            from_a: oa[old_range.start],
            to_a: oa[old_range.end],
            from_b: ob[new_range.start],
            to_b: ob[new_range.end],
        };
        match changes.last_mut() {
            Some(last) if last.to_a == change.from_a && last.to_b == change.from_b => {
                last.to_a = change.to_a;
                last.to_b = change.to_b;
            }
            _ => changes.push(change),
        }
    }
    changes
}

/// `presentableDiff`'s final `mergeAdjacent(changes, 3)`.
fn merge_adjacent(changes: &mut Vec<Change>, min_gap: usize) {
    let mut i = 1;
    while i < changes.len() {
        let (prev, cur) = (changes[i - 1], changes[i]);
        if prev.to_a + min_gap > cur.from_a && prev.to_b + min_gap > cur.from_b {
            changes[i - 1] = Change {
                from_a: prev.from_a,
                to_a: cur.to_a,
                from_b: prev.from_b,
                to_b: cur.to_b,
            };
            changes.remove(i);
        } else {
            i += 1;
        }
    }
}

/// `toChunks`: changes widened to whole lines, merging those that touch.
fn chunks(old: &Doc, new: &Doc, deadline: Option<Instant>) -> Vec<Change> {
    let mut changes = line_changes(old, new, deadline);
    merge_adjacent(&mut changes, 3);
    let from_line = |fa: usize, fb: usize| {
        let (la, lb) = (old.line_at(fa), new.line_at(fb));
        if old.line_to(la) == fa && new.line_to(lb) == fb && fa < old.len && fb < new.len {
            (fa + 1, fb + 1)
        } else {
            (old.starts[la], new.starts[lb])
        }
    };
    let to_line = |ta: usize, tb: usize| {
        let (la, lb) = (old.line_at(ta), new.line_at(tb));
        if old.starts[la] == ta && new.starts[lb] == tb {
            (ta, tb)
        } else {
            (old.line_to(la) + 1, new.line_to(lb) + 1)
        }
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < changes.len() {
        let change = changes[i];
        let (from_a, from_b) = from_line(change.from_a, change.from_b);
        let (mut to_a, mut to_b) = to_line(change.to_a, change.to_b);
        while i + 1 < changes.len() {
            let next = changes[i + 1];
            let (next_a, next_b) = from_line(next.from_a, next.from_b);
            if next_a > to_a + 1 && next_b > to_b + 1 {
                break;
            }
            (to_a, to_b) = to_line(next.to_a, next.to_b);
            i += 1;
        }
        out.push(Change {
            from_a,
            to_a: from_a.max(to_a),
            from_b,
            to_b: from_b.max(to_b),
        });
        i += 1;
    }
    out
}

/// unifiedLinesFromTexts.
fn unified_lines(
    original: &str,
    current: &str,
    deadline: Option<Instant>,
) -> (Vec<Line>, Vec<Change>) {
    let old = Doc::new(original);
    let new = Doc::new(current);
    let context = |from_b: usize, to_b: usize, from_a: usize| -> Vec<Line> {
        let inserted = new.lines_in_range(from_b, to_b);
        if inserted.is_empty() {
            return Vec::new();
        }
        let deleted = old.lines_in_range(from_a, from_a + (to_b - from_b));
        inserted
            .iter()
            .enumerate()
            .map(|(i, (number, text))| Line {
                kind: LineKind::Context,
                text: (*text).to_owned(),
                old_number: Some(deleted.get(i).map_or(*number, |d| d.0)),
                new_number: Some(*number),
                chunk: None,
            })
            .collect()
    };
    if original == current {
        return (context(0, new.len, 0), Vec::new());
    }
    let chunks = chunks(&old, &new, deadline);
    if chunks.is_empty() {
        return (context(0, new.len, 0), chunks);
    }
    let mut lines = Vec::new();
    let (mut old_pos, mut new_pos) = (0, 0);
    for (index, chunk) in chunks.iter().enumerate() {
        if new_pos < chunk.from_b {
            lines.extend(context(new_pos, chunk.from_b, old_pos));
        }
        for (number, text) in old.lines_in_range(chunk.from_a, chunk.to_a) {
            lines.push(Line {
                kind: LineKind::Del,
                text: text.to_owned(),
                old_number: Some(number),
                new_number: None,
                chunk: Some(index),
            });
        }
        for (number, text) in new.lines_in_range(chunk.from_b, chunk.to_b) {
            lines.push(Line {
                kind: LineKind::Add,
                text: text.to_owned(),
                old_number: None,
                new_number: Some(number),
                chunk: Some(index),
            });
        }
        old_pos = chunk.to_a;
        new_pos = chunk.to_b;
    }
    if new_pos < new.len {
        lines.extend(context(new_pos, new.len, old_pos));
    }
    (lines, chunks)
}

#[cfg(test)]
mod tests;
