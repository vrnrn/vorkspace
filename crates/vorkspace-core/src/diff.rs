//! The unified review's diff model and hunk staging. Files are compared line
//! by line (each line with its newline, as git does), then grouped into chunks exactly
//! as CodeMirror's `Chunk.build` with `presentableDiff` does: changes less
//! than three UTF-16 units apart merge, and chunks cover whole lines.
use similar::{capture_diff_deadline, Algorithm, DiffOp};
use std::time::{Duration, Instant};

pub const CONTEXT_DEFAULT: usize = 3;
/// The line pass budget. Past it, Myers replaces whatever range it
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
        line_count: usize,
    },
}
impl Block {
    pub fn lines(&self) -> &[Line] {
        match self {
            Self::Hunk { lines, .. } => lines,
            Self::Fold { .. } => &[],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UnifiedFile {
    pub additions: u32,
    pub deletions: u32,
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
    if original == current {
        let line_count = current.split_inclusive('\n').count();
        return UnifiedFile {
            blocks: (line_count > 0)
                .then_some(Block::Fold { line_count })
                .into_iter()
                .collect(),
            ..UnifiedFile::default()
        };
    }
    let old = Doc::new(original);
    let new = Doc::new(current);
    let ranges = chunks(&old, &new, deadline);
    let mut output = UnifiedFile {
        chunks: Vec::with_capacity(ranges.len()),
        ..UnifiedFile::default()
    };
    let mut pending_context = Vec::new();
    let mut has_preceding_change = false;
    let (mut old_pos, mut new_pos) = (0, 0);
    for (index, range) in ranges.into_iter().enumerate() {
        if new_pos < range.from_b {
            pending_context.push(ContextSegment::new(
                &old,
                &new,
                old_pos,
                new_pos..range.from_b,
            ));
        }
        let removed = old.line_range(range.from_a, range.to_a);
        let added = new.line_range(range.from_b, range.to_b);
        // A byte edit can have no displayed lines. Keep the context on both
        // sides of that edit in the same run until a visible change divides it.
        if !removed.is_empty() || !added.is_empty() {
            let leading = if has_preceding_change { context } else { 0 };
            output.append_context(&new, &pending_context, (leading, context));
            pending_context.clear();
            has_preceding_change = true;
            output.deletions += removed.len() as u32;
            output.additions += added.len() as u32;
            let removed = removed.map(|ix| Line {
                kind: LineKind::Del,
                text: old.lines[ix].to_owned(),
                old_number: Some((ix + 1) as u32),
                new_number: None,
                chunk: Some(index),
            });
            let added = added.map(|ix| Line {
                kind: LineKind::Add,
                text: new.lines[ix].to_owned(),
                old_number: None,
                new_number: Some((ix + 1) as u32),
                chunk: Some(index),
            });
            output.append_hunk(removed.chain(added), Some(index));
        }
        output.chunks.push(Change {
            replace_start: old.byte(range.from_a),
            replace_end: old.byte(range.to_a.min(old.len)),
            insert_start: new.byte(range.from_b),
            insert_end: new.byte(range.from_b.max(range.to_b.saturating_sub(1)).min(new.len)),
            add_newline: range.from_b != range.to_b && range.to_a <= old.len,
        });
        old_pos = range.to_a;
        new_pos = range.to_b;
    }
    if new_pos < new.len {
        pending_context.push(ContextSegment::new(&old, &new, old_pos, new_pos..new.len));
    }
    let leading = if has_preceding_change { context } else { 0 };
    output.append_context(&new, &pending_context, (leading, 0));
    output
}

struct ContextSegment {
    original: std::ops::Range<usize>,
    current: std::ops::Range<usize>,
}

impl ContextSegment {
    fn new(old: &Doc<'_>, new: &Doc<'_>, old_pos: usize, current: std::ops::Range<usize>) -> Self {
        Self {
            original: old.line_range(old_pos, old_pos + current.len()),
            current: new.line_range(current.start, current.end),
        }
    }
}

impl UnifiedFile {
    fn append_hunk(&mut self, lines: impl Iterator<Item = Line>, chunk: Option<usize>) {
        let mut lines = lines.peekable();
        if lines.peek().is_none() {
            return;
        }
        if let Some(Block::Hunk {
            lines: prior,
            chunk: first,
        }) = self.blocks.last_mut()
        {
            prior.extend(lines);
            *first = first.or(chunk);
        } else {
            self.blocks.push(Block::Hunk {
                lines: lines.collect(),
                chunk,
            });
        }
    }

    fn append_context(
        &mut self,
        new: &Doc<'_>,
        segments: &[ContextSegment],
        (leading, trailing): (usize, usize),
    ) {
        let count = segments.iter().map(|segment| segment.current.len()).sum();
        let leading = leading.min(count);
        let trailing = count - trailing.min(count - leading);
        let lines = |from: usize, to: usize| {
            let mut start = 0;
            segments.iter().flat_map(move |segment| {
                let len = segment.current.len();
                let from = from.saturating_sub(start).min(len);
                let to = to.saturating_sub(start).min(len);
                start += len;
                (from..to).map(move |offset| {
                    let ix = segment.current.start + offset;
                    let number = (ix + 1) as u32;
                    Line {
                        kind: LineKind::Context,
                        text: new.lines[ix].to_owned(),
                        old_number: Some(if offset < segment.original.len() {
                            (segment.original.start + offset + 1) as u32
                        } else {
                            number
                        }),
                        new_number: Some(number),
                        chunk: None,
                    }
                })
            })
        };
        self.append_hunk(lines(0, leading), None);
        if leading < trailing {
            self.blocks.push(Block::Fold {
                line_count: trailing - leading,
            });
        }
        self.append_hunk(lines(trailing, count), None);
    }
}

/// One displayed chunk stores its byte ranges against the two diff inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    replace_start: usize,
    replace_end: usize,
    insert_start: usize,
    insert_end: usize,
    add_newline: bool,
}

/// Apply the displayed chunk without diffing again or rebuilding line indexes.
pub fn stage_chunk(original: &str, current: &str, change: Change) -> String {
    let before = &original[..change.replace_start];
    let insert = &current[change.insert_start..change.insert_end];
    let after = &original[change.replace_end..];
    let mut text = String::with_capacity(
        before.len() + insert.len() + after.len() + usize::from(change.add_newline),
    );
    text.push_str(before);
    text.push_str(insert);
    if change.add_newline {
        text.push('\n');
    }
    text.push_str(after);
    text
}

/// Where a diff comment points: `path:line`.
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
    /// The indexes of the lines intersecting a UTF-16 range.
    fn line_range(&self, from: usize, to: usize) -> std::ops::Range<usize> {
        if from >= to || self.len == 0 {
            return 0..0;
        }
        let end = from.max(to - 1).min(self.len.saturating_sub(1));
        self.line_at(from.min(self.len))..self.line_at(end) + 1
    }
}
fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UnitChange {
    from_a: usize,
    to_a: usize,
    from_b: usize,
    to_b: usize,
}

/// Each line with its newline is one symbol; the result maps
/// back to UTF-16 offsets.
fn line_changes(old: &Doc, new: &Doc, deadline: Option<Instant>) -> Vec<UnitChange> {
    fn tokens(source: &str) -> Vec<&str> {
        source.split_inclusive('\n').collect()
    }
    let (a, b) = (tokens(old.source), tokens(new.source));
    let offset = |doc: &Doc<'_>, line| doc.starts.get(line).copied().unwrap_or(doc.len);
    let ops = capture_diff_deadline(Algorithm::Myers, &a, 0..a.len(), &b, 0..b.len(), deadline);
    let mut changes: Vec<UnitChange> = Vec::new();
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
        let change = UnitChange {
            from_a: offset(old, old_range.start),
            to_a: offset(old, old_range.end),
            from_b: offset(new, new_range.start),
            to_b: offset(new, new_range.end),
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
fn merge_adjacent(changes: &mut Vec<UnitChange>, min_gap: usize) {
    changes.dedup_by(|next, prior| {
        if prior.to_a.saturating_add(min_gap) > next.from_a
            && prior.to_b.saturating_add(min_gap) > next.from_b
        {
            prior.to_a = next.to_a;
            prior.to_b = next.to_b;
            true
        } else {
            false
        }
    });
}

/// `toChunks`: changes widened to whole lines, merging those that touch.
fn chunks(old: &Doc, new: &Doc, deadline: Option<Instant>) -> Vec<UnitChange> {
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
        out.push(UnitChange {
            from_a,
            to_a: from_a.max(to_a),
            from_b,
            to_b: from_b.max(to_b),
        });
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests;
