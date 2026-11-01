//! Text rules that match the JavaScript ones the persisted formats were built on.
/// JavaScript `\s`: Unicode white space plus the BOM.
pub(crate) fn js_space(ch: char) -> bool {
    ch.is_whitespace() || ch == '\u{feff}'
}

pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(js_space)
}

pub(crate) fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Longest prefix of at most `units` UTF-16 code units, without splitting a pair.
pub(crate) fn js_slice(text: &str, units: usize) -> &str {
    let mut count = 0;
    for (index, ch) in text.char_indices() {
        count += ch.len_utf16();
        if count > units {
            return &text[..index];
        }
    }
    text
}
