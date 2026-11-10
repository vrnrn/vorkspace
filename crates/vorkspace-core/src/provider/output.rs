//! Retain a bounded UTF-8 command tail without rebuilding it per provider delta.
use crate::tool::{OUTPUT_LIMIT, OUTPUT_OMISSION};

#[derive(Default)]
pub(super) struct OutputTail {
    text: String,
    head: usize,
    omitted: bool,
}

impl OutputTail {
    pub(super) fn from_output(text: String) -> Self {
        let omitted = text.len() > OUTPUT_LIMIT && text.starts_with(OUTPUT_OMISSION);
        let head = if omitted { OUTPUT_OMISSION.len() } else { 0 };
        Self {
            text,
            head,
            omitted,
        }
    }

    pub(super) fn append(&mut self, delta: &str) {
        if delta.len() >= OUTPUT_LIMIT {
            let head = boundary(delta, delta.len() - OUTPUT_LIMIT);
            self.omitted |= head > 0 || self.text.len() > self.head;
            self.text.clear();
            self.text.push_str(&delta[head..]);
            self.head = 0;
            return;
        }
        self.text.push_str(delta);
        let head = boundary(
            &self.text,
            self.text.len().saturating_sub(OUTPUT_LIMIT).max(self.head),
        );
        self.omitted |= head > self.head;
        self.head = head;
        if self.head >= OUTPUT_LIMIT {
            self.text.drain(..self.head);
            self.head = 0;
        }
    }

    pub(super) fn snapshot(&self) -> String {
        let retained = &self.text[self.head..];
        if !self.omitted {
            return retained.to_owned();
        }
        let mut output = String::with_capacity(OUTPUT_OMISSION.len() + retained.len());
        output.push_str(OUTPUT_OMISSION);
        output.push_str(retained);
        output
    }
}

fn boundary(text: &str, mut start: usize) -> usize {
    while !text.is_char_boundary(start) {
        start += 1;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_keeps_the_latest_complete_utf8_tail() {
        let mut stream = OutputTail::default();
        let mut full = String::new();
        for (count, delta) in [(3, "start "), (20_000, "🙂βx"), (500, " λ"), (1, " end")] {
            let delta = delta.repeat(count);
            stream.append(&delta);
            full.push_str(&delta);
            let start = full.ceil_char_boundary(full.len().saturating_sub(OUTPUT_LIMIT));
            let expected = if start > 0 {
                format!("{OUTPUT_OMISSION}{}", &full[start..])
            } else {
                full.clone()
            };
            assert_eq!(stream.snapshot(), expected);
        }
        for _ in 0..160 {
            stream.append("🙂xβ ".repeat(96).as_str());
        }
        assert!(stream.text.len() < OUTPUT_LIMIT * 2);
        assert!(stream.snapshot().starts_with(OUTPUT_OMISSION));
    }
}
