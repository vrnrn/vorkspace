//! Native Markdown model for AgentMarkdown's Streamdown elements.
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::{iter::Peekable, sync::Arc};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strikethrough: bool,
    pub link: Option<String>,
    pub task: Option<TaskMarker>,
    pub math: bool,
    pub search: Option<bool>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct TaskMarker {
    pub checked: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct InlineRun {
    pub text: String,
    pub style: InlineStyle,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TableAlign {
    Left,
    Center,
    Right,
}
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::enum_variant_names)] // Markdown terms: code block and block quote.
pub enum Block {
    Paragraph {
        runs: Vec<InlineRun>,
    },
    Heading {
        level: u8,
        runs: Vec<InlineRun>,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
        /// False for a fence still open at the end of the text, such as one
        /// arriving in a streamed reply.
        closed: bool,
    },
    List {
        ordered_start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    BlockQuote {
        children: Vec<Block>,
    },
    Table {
        header: Vec<Vec<InlineRun>>,
        rows: Vec<Vec<Vec<InlineRun>>>,
        align: Vec<TableAlign>,
    },
    Rule,
    Math {
        tex: String,
    },
}
#[derive(Clone)]
pub struct TopBlock {
    pub block: Block,
}
#[derive(Clone, Default)]
pub struct BlockTree {
    pub blocks: Vec<Arc<TopBlock>>,
}
#[derive(Default)]
pub struct MarkdownParser {
    source: String,
    tree: BlockTree,
}
impl MarkdownParser {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_text(&mut self, text: &str) {
        if self.source != text {
            self.source = text.into();
            self.tree = parse_full(text);
        }
    }
    pub fn tree(&self) -> &BlockTree {
        &self.tree
    }
    pub fn display_tree(&self) -> BlockTree {
        parse_full(&mend_stream(&self.source))
    }
}

// streamingMarkdown.ts / remend: repair prose blocks while leaving literal
// fences untouched. Repairs affect only the display tree, never stored text.
fn mend_stream(source: &str) -> String {
    let mut out = String::with_capacity(source.len() + 32);
    let mut prose = String::new();
    let mut fence = None;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        let marker = trimmed.as_bytes().first().copied().unwrap_or(0);
        let width = trimmed.bytes().take_while(|b| *b == marker).count();
        let is_fence = indent <= 3 && matches!(marker, b'`' | b'~') && width >= 3;
        if let Some((opened, count)) = fence {
            out.push_str(line);
            if is_fence && marker == opened && width >= count && trimmed[width..].trim().is_empty()
            {
                fence = None;
            }
        } else if is_fence {
            out.push_str(&mend_prose(&prose));
            prose.clear();
            out.push_str(line);
            fence = Some((marker, width));
        } else {
            prose.push_str(line);
            if line.trim().is_empty() {
                out.push_str(&mend_prose(&prose));
                prose.clear();
            }
        }
    }
    out.push_str(&mend_prose(&prose));
    out
}
fn mend_prose(source: &str) -> String {
    let mut text = source.to_owned();
    let mut stack: Vec<String> = Vec::new();
    let mut brackets = Vec::new();
    let mut code = None;
    let mut at = 0;
    let mut line_checked = 0;
    let mut line_blank = true;
    while at < text.len() {
        let byte = text.as_bytes()[at];
        if byte == b'\\' {
            at += 1;
            if at < text.len() {
                at += text[at..].chars().next().unwrap().len_utf8();
            }
            continue;
        }
        if byte == b'`' {
            let n = text[at..].bytes().take_while(|b| *b == b'`').count();
            match code {
                Some(width) if width == n => code = None,
                None => code = Some(n),
                _ => {}
            }
            at += n;
            continue;
        }
        if code.is_some() {
            at += text[at..].chars().next().unwrap().len_utf8();
            continue;
        }
        if byte == b'<'
            && !text[at..].contains('>')
            && text[at + 1..]
                .trim_start_matches('/')
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
        {
            text.truncate(at);
            break;
        }
        if byte == b'[' {
            brackets.push(at);
        }
        if byte == b']'
            && let Some(open) = brackets.pop()
            && text.as_bytes().get(at + 1) == Some(&b'(')
        {
            let mut depth = 1;
            let mut end = at + 2;
            while end < text.len() {
                match text.as_bytes()[end] {
                    b'\\' => {
                        end = (end + 2).min(text.len());
                        continue;
                    }
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                end += 1;
            }
            if depth != 0 {
                if open > 0 && text.as_bytes()[open - 1] == b'!' {
                    text.truncate(open - 1);
                } else {
                    text.truncate(at + 2);
                    text.push_str("streamdown:incomplete-link)");
                }
                break;
            }
            at = end + 1;
            continue;
        }
        if matches!(byte, b'*' | b'_' | b'~') {
            let n = text[at..].bytes().take_while(|b| *b == byte).count();
            let previous = text[..at].chars().next_back();
            let next = text[at + n..].chars().next();
            // Scan each prefix byte only once, including text skipped by
            // escapes, code and links. Rescanning the line at every delimiter
            // makes long lines of identifiers quadratic during live repair.
            for ch in text[line_checked..at].chars() {
                line_blank = if ch == '\n' {
                    true
                } else {
                    line_blank && ch.is_whitespace()
                };
            }
            line_checked = at;
            let rule = n >= 3
                && line_blank
                && text[at..]
                    .split('\n')
                    .next()
                    .unwrap_or("")
                    .chars()
                    .all(|c| c == byte as char || c.is_whitespace());
            let list = line_blank && n == 1 && byte == b'*' && next.is_none_or(char::is_whitespace);
            let intraword = byte == b'_'
                && previous.is_some_and(char::is_alphanumeric)
                && next.is_some_and(char::is_alphanumeric);
            if !rule && !list && !intraword && (byte != b'~' || n == 2) && n <= 3 {
                let delimiter = text[at..at + n].to_owned();
                if previous.is_some_and(|c| !c.is_whitespace()) && stack.last() == Some(&delimiter)
                {
                    stack.pop();
                } else if next.is_some_and(|c| !c.is_whitespace()) {
                    stack.push(delimiter);
                }
            }
            at += n;
        } else {
            at += text[at..].chars().next().unwrap().len_utf8();
        }
    }
    if code.is_none()
        && let Some(open) = brackets.last().copied()
    {
        if open > 0 && text.as_bytes()[open - 1] == b'!' {
            text.truncate(open - 1);
        } else if !text[open + 1..].trim().is_empty() {
            text.push_str("](streamdown:incomplete-link)");
        }
    }
    let whitespace = text.len() - text.trim_end().len();
    let tail = text.split_off(text.len() - whitespace);
    if let Some(width) = code {
        text.push_str(&"`".repeat(width));
    }
    for delimiter in stack.into_iter().rev() {
        text.push_str(&delimiter);
    }
    text.push_str(&tail);
    let last = text.trim_end().rsplit('\n').next().unwrap_or("");
    if matches!(last.trim(), "-" | "--" | "=" | "==") && text.trim_end().contains('\n') {
        let at = text.trim_end().len();
        text.insert(at, '\u{200b}');
    }
    text
}
type Events<'a> = Peekable<std::vec::IntoIter<Event<'a>>>;
pub fn parse_full(text: &str) -> BlockTree {
    parse_with_breaks(text, false)
}

/// Notes preserve prose line breaks, matching AgentMarkdown's hardBreaks flag.
pub fn parse_full_hard_breaks(text: &str) -> BlockTree {
    parse_with_breaks(text, true)
}

fn parse_with_breaks(text: &str, hard_breaks: bool) -> BlockTree {
    // Whether each code block, in document order, is complete. Nesting
    // mirrors `blocks`, which skips blocks 32 containers deep.
    let mut complete = Vec::new();
    let mut depth = 0usize;
    let mut events = Parser::new_ext(
        text,
        Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_MATH,
    )
    .into_offset_iter()
    .map(|(event, range)| {
        match &event {
            Event::Start(Tag::BlockQuote(_) | Tag::Item) => depth += 1,
            Event::End(TagEnd::BlockQuote(_) | TagEnd::Item) => depth = depth.saturating_sub(1),
            Event::Start(Tag::CodeBlock(kind)) if depth < 32 => complete.push(match kind {
                CodeBlockKind::Fenced(_) => fence_complete(text, range),
                CodeBlockKind::Indented => true,
            }),
            _ => {}
        }
        if hard_breaks && matches!(event, Event::SoftBreak) {
            Event::HardBreak
        } else {
            event
        }
    })
    .collect::<Vec<_>>()
    .into_iter()
    .peekable();
    let mut blocks = blocks(&mut events, None, 0);
    mark_complete(&mut blocks, &mut complete.into_iter());
    BlockTree {
        blocks: blocks
            .into_iter()
            .map(|block| Arc::new(TopBlock { block }))
            .collect(),
    }
}
/// pulldown-cmark closes a fence at the end of the text, so a fence that
/// reaches the end may still be arriving: only its closing fence line proves
/// it complete. One that ends earlier, closed by its fence or by the end of
/// its list item or quote, can't grow.
fn fence_complete(text: &str, range: std::ops::Range<usize>) -> bool {
    let end = text.trim_end().len();
    if range.end < end {
        return true;
    }
    let source = text[range.start..end.max(range.start)].trim_end();
    let mut lines = source.lines();
    lines.next();
    lines.last().is_some_and(|last| {
        // Inside a quote, the line carries its markers.
        let last = last.trim_start_matches(|c: char| c == '>' || c.is_whitespace());
        last.len() >= 3 && (last.chars().all(|c| c == '`') || last.chars().all(|c| c == '~'))
    })
}

fn mark_complete(blocks: &mut [Block], complete: &mut impl Iterator<Item = bool>) {
    for block in blocks {
        match block {
            Block::CodeBlock { closed, .. } => *closed = complete.next().unwrap_or(true),
            Block::BlockQuote { children } => mark_complete(children, complete),
            Block::List { items, .. } => {
                for item in items {
                    mark_complete(item, complete);
                }
            }
            _ => {}
        }
    }
}

fn blocks(events: &mut Events<'_>, until: Option<TagEnd>, depth: usize) -> Vec<Block> {
    let mut out = Vec::new();
    while let Some(event) = events.next() {
        match event {
            Event::End(end) if until == Some(end) => break,
            Event::Start(tag) if depth < 32 => match tag {
                Tag::Paragraph => out.push(Block::Paragraph {
                    runs: inlines(events, TagEnd::Paragraph),
                }),
                Tag::Heading { level, .. } => {
                    let number = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    out.push(Block::Heading {
                        level: number,
                        runs: inlines(events, TagEnd::Heading(level)),
                    });
                }
                Tag::CodeBlock(kind) => {
                    let language = match kind {
                        CodeBlockKind::Fenced(info) if !info.is_empty() => Some(info.into_string()),
                        _ => None,
                    };
                    let mut code = String::new();
                    for event in events.by_ref() {
                        match event {
                            Event::End(TagEnd::CodeBlock) => break,
                            Event::Text(text) => code.push_str(&text),
                            _ => {}
                        }
                    }
                    out.push(Block::CodeBlock {
                        language,
                        code,
                        closed: true,
                    });
                }
                Tag::BlockQuote(kind) => out.push(Block::BlockQuote {
                    children: blocks(events, Some(TagEnd::BlockQuote(kind)), depth + 1),
                }),
                Tag::List(start) => {
                    let mut items = Vec::new();
                    while let Some(event) = events.next() {
                        match event {
                            Event::End(TagEnd::List(_)) => break,
                            Event::Start(Tag::Item) => {
                                items.push(blocks(events, Some(TagEnd::Item), depth + 1))
                            }
                            _ => {}
                        }
                    }
                    out.push(Block::List {
                        ordered_start: start,
                        items,
                    });
                }
                Tag::Table(alignment) => {
                    let align = alignment
                        .into_iter()
                        .map(|a| match a {
                            Alignment::Center => TableAlign::Center,
                            Alignment::Right => TableAlign::Right,
                            _ => TableAlign::Left,
                        })
                        .collect();
                    let mut rows = Vec::new();
                    let mut row = Vec::new();
                    let mut header = Vec::new();
                    let mut head = false;
                    while let Some(event) = events.next() {
                        match event {
                            Event::End(TagEnd::Table) => break,
                            Event::Start(Tag::TableHead) => head = true,
                            Event::Start(Tag::TableCell) => {
                                row.push(inlines(events, TagEnd::TableCell))
                            }
                            Event::End(TagEnd::TableHead) => {
                                header = std::mem::take(&mut row);
                                head = false;
                            }
                            Event::End(TagEnd::TableRow) if !head => {
                                rows.push(std::mem::take(&mut row))
                            }
                            _ => {}
                        }
                    }
                    out.push(Block::Table {
                        header,
                        rows,
                        align,
                    });
                }
                tag @ (Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. }) => {
                    let mut runs = Vec::new();
                    inline_span(events, tag, InlineStyle::default(), &mut runs);
                    if let Some(Block::Paragraph { runs: previous }) = out.last_mut() {
                        previous.extend(runs);
                    } else {
                        out.push(Block::Paragraph { runs });
                    }
                }
                _ => {}
            },
            Event::Rule => out.push(Block::Rule),
            Event::DisplayMath(tex) => out.push(Block::Math {
                tex: tex.into_string(),
            }),
            // Tight list items have no paragraph event around their inline runs.
            event @ (Event::Text(_)
            | Event::Code(_)
            | Event::TaskListMarker(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::InlineMath(_)) => {
                let mut runs = vec![];
                append_run(&mut runs, event, InlineStyle::default());
                while let Some(next) = events.peek() {
                    if matches!(
                        next,
                        Event::Start(
                            Tag::List(_) | Tag::BlockQuote(_) | Tag::CodeBlock(_) | Tag::Paragraph
                        ) | Event::End(TagEnd::Item)
                    ) {
                        break;
                    }
                    if matches!(next, Event::End(_)) {
                        break;
                    }
                    let next = events.next().unwrap();
                    match next {
                        Event::Start(tag) => {
                            inline_span(events, tag, InlineStyle::default(), &mut runs)
                        }
                        event => append_run(&mut runs, event, InlineStyle::default()),
                    }
                }
                if let Some(Block::Paragraph { runs: previous }) = out.last_mut() {
                    previous.extend(runs);
                } else {
                    out.push(Block::Paragraph { runs });
                }
            }
            Event::Start(
                tag @ (Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. }),
            ) => {
                let mut runs = vec![];
                inline_span(events, tag, InlineStyle::default(), &mut runs);
                if let Some(Block::Paragraph { runs: previous }) = out.last_mut() {
                    previous.extend(runs);
                } else {
                    out.push(Block::Paragraph { runs });
                }
            }
            _ => {}
        }
    }
    out
}
fn inlines(events: &mut Events<'_>, end: TagEnd) -> Vec<InlineRun> {
    let mut out = Vec::new();
    while let Some(event) = events.next() {
        match event {
            Event::End(tag) if tag == end => break,
            Event::Start(tag) => inline_span(events, tag, InlineStyle::default(), &mut out),
            event => append_run(&mut out, event, InlineStyle::default()),
        }
    }
    out
}
fn inline_span(
    events: &mut Events<'_>,
    tag: Tag<'_>,
    mut style: InlineStyle,
    out: &mut Vec<InlineRun>,
) {
    let end = tag.to_end();
    match tag {
        Tag::Strong => style.bold = true,
        Tag::Emphasis => style.italic = true,
        Tag::Strikethrough => style.strikethrough = true,
        Tag::Link { dest_url, .. } if dest_url.as_ref() != "streamdown:incomplete-link" => {
            style.link = Some(dest_url.into_string());
        }
        _ => {}
    }
    while let Some(event) = events.next() {
        match event {
            Event::End(tag) if tag == end => break,
            Event::Start(tag) => inline_span(events, tag, style.clone(), out),
            event => append_run(out, event, style.clone()),
        }
    }
}
fn append_run(out: &mut Vec<InlineRun>, event: Event<'_>, mut style: InlineStyle) {
    let text = match event {
        Event::Text(text) => text.into_string(),
        Event::Code(text) => {
            style.code = true;
            text.into_string()
        }
        Event::SoftBreak => " ".into(),
        Event::HardBreak => "\n".into(),
        Event::TaskListMarker(checked) => {
            style.task = Some(TaskMarker { checked });
            "\u{200b}".into()
        }
        Event::InlineMath(text) => {
            style.math = true;
            text.into_string()
        }
        _ => return,
    };
    out.push(InlineRun { text, style });
}

#[cfg(test)]
mod note_break_tests {
    use super::*;

    #[test]
    fn notes_preserve_prose_breaks_without_changing_chat_or_code() {
        fn prose(tree: &BlockTree) -> String {
            let Block::Paragraph { runs } = &tree.blocks[0].block else {
                panic!("expected paragraph")
            };
            runs.iter().map(|run| run.text.as_str()).collect()
        }
        assert_eq!(prose(&parse_full("one\ntwo")), "one two");
        assert_eq!(prose(&parse_full_hard_breaks("one\ntwo")), "one\ntwo");
        let tree = parse_full_hard_breaks("```text\none\ntwo\n```");
        let Block::CodeBlock { code, .. } = &tree.blocks[0].block else {
            panic!("expected code")
        };
        assert_eq!(code, "one\ntwo\n");
    }
}

#[cfg(test)]
mod repair_tests {
    use super::*;

    #[test]
    fn line_context_survives_escaped_newlines_code_links_and_unicode_whitespace() {
        for (source, expected) in [
            ("identifier_name *tail", "identifier_name *tail*"),
            ("prefix\n\t* item\n**tail", "prefix\n\t* item\n**tail**"),
            ("prefix\\\n* item", "prefix\\\n* item"),
            ("`code\n_` **tail", "`code\n_` **tail**"),
            (
                "[label](path\nvalue_name) *tail",
                "[label](path\nvalue_name) *tail*",
            ),
            (" \u{2003}___\n**tail", " \u{2003}___\n**tail**"),
            ("prefix\u{2028}***", "prefix\u{2028}***"),
            (" \t*** \n* item", " \t*** \n* item"),
        ] {
            assert_eq!(mend_prose(source), expected, "source={source:?}");
        }
    }

    #[test]
    fn long_identifier_lines_keep_literal_underscores_and_streaming_repairs() {
        let line = "synthetic_value_name ".repeat(25_000);
        assert_eq!(mend_prose(&line), line);
        let unfinished = format!("{line}**tail");
        assert_eq!(mend_prose(&unfinished), format!("{unfinished}**"));
        let fenced = format!("```text\n{unfinished}\n```\n");
        assert_eq!(mend_stream(&fenced), fenced);
    }

    #[test]
    #[ignore = "explicit live Markdown repair CPU scaling measurement"]
    fn repair_scaling() {
        for bytes in [16_384, 65_536, 262_144, 524_288] {
            let source = "synthetic_value_name ".repeat(bytes / 21);
            for sample in 0..3 {
                let start = std::time::Instant::now();
                let repaired = mend_prose(std::hint::black_box(&source));
                let elapsed = start.elapsed();
                assert_eq!(repaired, source);
                eprintln!(
                    "repair bytes={} sample={sample} elapsed_us={}",
                    source.len(),
                    elapsed.as_micros()
                );
            }
        }
    }
}
