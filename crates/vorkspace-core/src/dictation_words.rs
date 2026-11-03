//! Validated local speech aliases and an append-only streaming text corrector.
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
use unicode_segmentation::UnicodeSegmentation;

const MAX_RULES: usize = 64;
const MAX_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub heard: String,
    pub preferred: String,
    key: String,
}

fn normalized(s: &str) -> String {
    unicase::UniCase::new(s).to_folded_case().nfc().collect()
}

pub fn word_character(c: char) -> bool {
    c.is_alphanumeric() || is_combining_mark(c)
}
fn word(s: &str) -> bool {
    s.chars().any(word_character)
}

fn units(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for g in s.graphemes(true) {
        let part = if g.chars().all(char::is_whitespace) {
            " ".to_owned()
        } else {
            normalized(g)
        };
        if part != " " || out.last().is_none_or(|last| last != " ") {
            out.push(part);
        }
    }
    out
}

/// One preferred term or `heard => preferred` rule per line. Text is persisted verbatim so
/// editing it does not discard spelling or comments by silently repairing it.
pub fn parse(text: &str) -> Result<Vec<Rule>, String> {
    let mut rules: Vec<Rule> = Vec::new();
    for (line, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let (heard, preferred) = raw.split_once("=>").unwrap_or((raw, raw));
        let (heard, preferred) = (heard.trim(), preferred.trim());
        if preferred.contains("=>") {
            return Err(format!("Line {}: use at most one =>.", line + 1));
        }
        if heard.is_empty() || preferred.is_empty() {
            return Err(format!("Line {}: enter both sides of the rule.", line + 1));
        }
        if heard.len() > MAX_BYTES || preferred.len() > MAX_BYTES {
            return Err(format!(
                "Line {}: each side must be at most {MAX_BYTES} bytes.",
                line + 1
            ));
        }
        if heard.chars().chain(preferred.chars()).any(char::is_control) {
            return Err(format!(
                "Line {}: control characters are not supported.",
                line + 1
            ));
        }
        if !heard.chars().any(char::is_alphanumeric) {
            return Err(format!(
                "Line {}: the heard phrase needs a letter or number.",
                line + 1
            ));
        }
        let key = units(heard).concat();
        if rules.iter().any(|rule| rule.key == key) {
            return Err(format!(
                "Line {}: this heard phrase is already present.",
                line + 1
            ));
        }
        if rules.len() == MAX_RULES {
            return Err(format!("Use at most {MAX_RULES} rules."));
        }
        rules.push(Rule {
            heard: heard.into(),
            preferred: preferred.into(),
            key,
        });
    }
    Ok(rules)
}

enum Match {
    No,
    Partial,
    Complete(usize),
}

/// Holds only text whose meaning might still change when another chunk arrives.
/// Matches case-insensitively at word edges, preserving surrounding punctuation.
/// The longest overlapping rule wins. Unfinished graphemes and phrases remain
/// private until a later chunk or `finish`; cancellation drops them.
pub struct Corrector {
    rules: Vec<Rule>,
    pending: String,
    previous_word: bool,
}

impl Corrector {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self {
            rules,
            pending: String::new(),
            previous_word: false,
        }
    }

    pub fn push(&mut self, text: &str, finish: bool) -> String {
        self.push_inner(text, finish, false, false)
    }

    /// A stable but not necessarily complete word can be released only when
    /// it cannot begin any configured alias. Potential matches still wait.
    pub fn push_stable(&mut self, text: &str) -> String {
        self.push_inner(text, false, false, true)
    }

    /// The producer has ended a complete word/segment and guarantees that a
    /// later word starts at a boundary. Longer possible phrases still wait.
    pub fn push_segment(&mut self, text: &str, finish: bool) -> String {
        self.push_inner(text, finish, true, false)
    }

    fn push_inner(&mut self, text: &str, finish: bool, closed: bool, stable: bool) -> String {
        self.pending.push_str(text);
        if self.rules.is_empty() {
            return std::mem::take(&mut self.pending);
        }
        let mut output = String::new();
        loop {
            let graphemes: Vec<(usize, &str)> = self.pending.grapheme_indices(true).collect();
            let mut available = if finish || closed || stable {
                graphemes.len()
            } else {
                graphemes.len().saturating_sub(1)
            };
            // A decoder can stop a chunk in the middle of a word, and the
            // last grapheme can still acquire combining marks. Keep the whole
            // unfinished word, rather than inserting all but its final letter.
            if !finish && !closed && !stable && graphemes.last().is_some_and(|g| word(g.1)) {
                while available > 0 && word(graphemes[available - 1].1) {
                    available -= 1;
                }
            }
            if available == 0 {
                break;
            }
            let mut best: Option<(usize, &str)> = None;
            let mut partial = false;
            for rule in &self.rules {
                match match_rule(
                    rule,
                    &graphemes,
                    available,
                    finish,
                    closed,
                    stable,
                    self.previous_word,
                ) {
                    Match::No => {}
                    Match::Partial => partial = true,
                    Match::Complete(end) => {
                        if best.is_none_or(|(n, _)| end > n) {
                            best = Some((end, &rule.preferred));
                        }
                    }
                }
            }
            if partial {
                break;
            }
            let (count, replacement) = best.unwrap_or((1, graphemes[0].1));
            output.push_str(replacement);
            self.previous_word = word(graphemes[count - 1].1);
            let end = graphemes.get(count).map_or(self.pending.len(), |g| g.0);
            self.pending.drain(..end);
        }
        output
    }
}

fn match_rule(
    rule: &Rule,
    input: &[(usize, &str)],
    available: usize,
    finish: bool,
    closed: bool,
    stable: bool,
    previous_word: bool,
) -> Match {
    if previous_word && rule.heard.graphemes(true).next().is_some_and(word) {
        return Match::No;
    }
    let mut index = 0;
    let mut remaining = rule.key.as_str();
    while !remaining.is_empty() {
        if index == available {
            return if finish { Match::No } else { Match::Partial };
        }
        if let Some(rest) = remaining.strip_prefix(' ') {
            if !input[index].1.chars().all(char::is_whitespace) {
                return Match::No;
            }
            while index < available && input[index].1.chars().all(char::is_whitespace) {
                index += 1;
            }
            remaining = rest;
        } else {
            let actual = normalized(input[index].1);
            let Some(rest) = remaining.strip_prefix(actual.as_str()) else {
                // A future combining mark can still complete the final
                // grapheme, e.g. `cafe` followed by the accent in `café`.
                if stable
                    && index + 1 == available
                    && remaining
                        .nfd()
                        .collect::<String>()
                        .starts_with(&actual.nfd().collect::<String>())
                {
                    return Match::Partial;
                }
                return Match::No;
            };
            remaining = rest;
            index += 1;
        }
    }
    if rule.heard.graphemes(true).next_back().is_some_and(word) {
        if index == available && !finish && !closed {
            return Match::Partial;
        }
        if index < available && word(input[index].1) {
            return Match::No;
        }
    }
    Match::Complete(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corrected(rules: &str, chunks: &[&str]) -> String {
        let mut corrector = Corrector::new(parse(rules).unwrap());
        let mut result = String::new();
        for chunk in chunks {
            result.push_str(&corrector.push(chunk, false));
        }
        result.push_str(&corrector.push("", true));
        result
    }

    #[test]
    fn every_character_boundary_matches_the_whole_text_reference() {
        let rules = "work space => Vorkspace\nnew york => NY\nnew york city => NYC\ncafé => Café\nc++ => C++";
        let text = "A WORK  space, New York City! Cafe\u{301}; c++ and workspace.";
        let expected = "A Vorkspace, NYC! Café; C++ and workspace.";
        assert_eq!(corrected(rules, &[text]), expected);
        let chars: Vec<&str> = text
            .char_indices()
            .map(|(i, c)| &text[i..i + c.len_utf8()])
            .collect();
        assert_eq!(corrected(rules, &chars), expected);
        for (index, _) in text.char_indices() {
            assert_eq!(
                corrected(rules, &[&text[..index], &text[index..]]),
                expected,
                "split at {index}"
            );
        }
    }

    #[test]
    fn pending_overlaps_wait_and_cancellation_drops_unpublished_text() {
        let rules = parse("new york => NY\nnew york city => NYC").unwrap();
        let mut c = Corrector::new(rules.clone());
        assert_eq!(c.push("New York ", false), "");
        assert_eq!(c.push("City!", false), "");
        assert_eq!(c.push(" next", false), "NYC! ");
        assert_eq!(c.push("", true), "next");
        let mut cancelled = Corrector::new(rules);
        assert_eq!(cancelled.push("New York ", false), "");
        drop(cancelled);
        assert_eq!(corrected("new york => NY", &["new yorker"]), "new yorker");
    }

    #[test]
    fn invalid_and_ambiguous_rules_are_rejected() {
        for bad in [
            "...",
            "=> name",
            "name =>",
            "a => b => c",
            "... => name",
            "É => one\ne\u{301} => two",
            "x => a\0b",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(parse("").unwrap().is_empty());
        assert!(parse("Vorkspace\nvorkspace => another").is_err());
        assert_eq!(corrected("Vorkspace", &["vork", "space!"]), "Vorkspace!");
    }

    #[test]
    fn unicode_case_folding_handles_expansions_and_greek_final_sigma() {
        assert_eq!(
            corrected("Straße => Straße\nκόσμος => Κόσμος", &["STRASSE ΚΌΣΜΟΣ!"]),
            "Straße Κόσμος!"
        );
        assert!(parse("Straße => one\nSTRASSE => two").is_err());
    }
    #[test]
    fn completed_segments_release_unrelated_last_words_but_hold_cross_pause_aliases() {
        let mut c = Corrector::new(parse("new york => NY\nnew york city => NYC").unwrap());
        assert_eq!(c.push_segment("hello", false), "hello");
        assert_eq!(c.push_segment(" New York", false), " ");
        assert_eq!(c.push_segment(" City", false), "NYC");
        assert_eq!(c.push("!", true), "!");
    }
    #[test]
    fn stable_unmatched_last_word_is_released_without_guessing_alias_boundaries() {
        let mut c = Corrector::new(parse("work space => Vorkspace\ncafé => Café").unwrap());
        assert_eq!(c.push_stable("country"), "country");
        assert_eq!(c.push_stable(" work"), " ");
        assert_eq!(c.push(" space ", false), "");
        assert_eq!(c.push_stable(""), "Vorkspace ");
        assert_eq!(c.push_stable("cafe"), "");
        assert_eq!(c.push("\u{301}!", true), "Café!");
    }
}
