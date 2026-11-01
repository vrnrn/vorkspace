//! AgentTranscript's DaySeparator and opensNewStretch, in the reader's local time.
use super::*;
use chrono::{DateTime, Datelike, Local, NaiveDate};

pub(super) fn full_stamp(at: u64) -> String {
    i64::try_from(at)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
        .map(|date| date.with_timezone(&Local).format("%c").to_string())
        .unwrap_or_default()
}

fn local_day(at: u64) -> Option<NaiveDate> {
    DateTime::from_timestamp_millis(i64::try_from(at).ok()?)
        .map(|date| date.with_timezone(&Local).date_naive())
}

pub(crate) fn opens_stretch(at: Option<u64>, previous: Option<u64>) -> bool {
    let Some(at) = at.filter(|at| local_day(*at).is_some()) else {
        return false;
    };
    previous.is_none_or(|previous| {
        at.saturating_sub(previous) > 60 * 60 * 1000 || local_day(at) != local_day(previous)
    })
}

fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    if date == today {
        "Today".into()
    } else if Some(date) == today.pred_opt() {
        "Yesterday".into()
    } else if date.year() == today.year() {
        date.format("%a, %b %-d").to_string()
    } else {
        date.format("%a, %b %-d, %Y").to_string()
    }
}

fn separator(at: u64) -> Option<gpui::AnyElement> {
    let day = day_label(local_day(at)?, Local::now().date_naive());
    let time = vorkspace_core::response::local_clock(at);
    Some(
        div()
            .id(SharedString::from(format!("bot-day-separator-{at}")))
            .debug_selector(|| "bot-day-separator".into())
            .role(Role::Label)
            .aria_label(format!("{day} {time}"))
            .w_full()
            .flex()
            .justify_center()
            .px_4()
            .pt_6()
            .pb_3()
            .text_size(px(12.))
            .line_height(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_full()
                    .bg(content(0.07))
                    .px_3()
                    .py_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(content(0.75))
                            .child(day),
                    )
                    .child(div().text_color(content(0.45)).child(time)),
            )
            .into_any_element(),
    )
}

/// When the message before `start` was sent: the previous prompt, or the
/// conversation's first row.
pub(crate) fn previous_send(source: &[Row], start: usize) -> Option<u64> {
    source[..start]
        .iter()
        .enumerate()
        .rfind(|(ix, row)| row.first && (row.is_prompt() || *ix == 0))
        .and_then(|(_, row)| row.sent_at)
}

pub(crate) fn bot_opens_stretch(source: &[Row], start: usize) -> bool {
    opens_stretch(source[start].sent_at, previous_send(source, start))
}

impl TranscriptView {
    fn previous_send(&self, start: usize) -> Option<u64> {
        previous_send(&self.source, start)
    }

    pub(super) fn bot_opens_stretch(&self, start: usize) -> bool {
        bot_opens_stretch(&self.source, start)
    }

    /// A user opens its stretch above its bubble. A standalone/internal run
    /// opens it above the Bot's header, since its prompt is hidden.
    fn bot_stamp(&self, start: usize) -> Option<u64> {
        if !self.bot || self.document {
            return None;
        }
        let row = self.source.get(start)?;
        let stamp = if row.role == "user" || self.peer_at(start).is_some() {
            start
        } else if row.is_prompt() {
            return None;
        } else {
            let turn = self.source[..start]
                .iter()
                .rposition(|r| r.is_prompt())
                .map_or(0, |ix| ix + 1);
            if start != turn {
                return None;
            }
            let prompt = self.prompt_start(turn);
            if self.peer_at(prompt).is_some()
                || self.source[prompt..turn].iter().any(|r| r.role == "user")
            {
                return None;
            }
            prompt
        };
        self.bot_opens_stretch(stamp)
            .then_some(self.source[stamp].sent_at)
            .flatten()
    }

    pub(super) fn bot_separator(&self, start: usize) -> Option<gpui::AnyElement> {
        self.bot_stamp(start).and_then(separator)
    }

    pub(super) fn pending_separator(&self, index: usize) -> Option<gpui::AnyElement> {
        let at = self.pending.get(index)?.sent_at;
        let previous = if index == 0 {
            self.previous_send(self.source.len())
        } else {
            self.pending[index - 1].sent_at
        };
        opens_stretch(at, previous)
            .then_some(at)
            .flatten()
            .and_then(separator)
    }
}

#[cfg(test)]
mod tests;
