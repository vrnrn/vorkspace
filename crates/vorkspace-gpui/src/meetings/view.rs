//! Recording controls and transcript presentation embedded in Notes.
use super::{Page, document::Document, live::Live};
use crate::{
    controls::page,
    popover::{Anchor, popover},
};
use gpui::{AnyElement, Context, Role, div, prelude::*, px};
use vorkspace_meetings::recording::{CaptureSource, Phase, State};
use vorkspace_ui::theme::icon;

impl Page {
    pub(super) fn record_control(&self, doc: &Document, cx: &mut Context<Self>) -> AnyElement {
        let id = doc.id().to_owned();
        let active = self.live.as_ref().filter(|live| live.state.phase.active());
        let selected = active.filter(|live| live.state.id == id);
        let run = selected.and_then(|live| live.state.run.clone());
        let (label, disabled) = if let Some(live) = selected {
            if live.state.phase == Phase::Finishing {
                ("Finishing…", true)
            } else {
                ("Stop recording", run.is_none())
            }
        } else if self.preparing.as_deref() == Some(id.as_str()) {
            ("Preparing…", true)
        } else {
            (
                "Start recording",
                !self.editable()
                    || self.preparing.is_some()
                    || self.saving
                    || self.analysis.is_some()
                    || active.is_some(),
            )
        };
        let control = if selected.is_some() {
            page::secondary("meeting-record", label, true, disabled)
                .child(icon("stop", page::ICON_SIZE))
        } else {
            page::primary("meeting-record", label, disabled)
        };
        control
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                if !disabled {
                    this.request_record(&id, run.as_deref(), cx);
                }
            }))
            .into_any_element()
    }

    pub(super) fn source_picker(
        &self,
        doc: &Document,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_source(doc);
        page::secondary(
            "meeting-audio-source",
            audio_source_label(selected),
            false,
            !enabled,
        )
        .relative()
        .border_1()
        .border_color(page::divider())
        .child(self.source_anchor.probe())
        .child(audio_source_label(selected))
        .child(icon("chevron-down", 11.))
        .on_click(cx.listener(move |this, _, _, cx| {
            if enabled {
                this.source_open = !this.source_open;
                cx.notify();
            }
        }))
        .into_any_element()
    }

    pub(super) fn source_menu(&self, doc: &Document, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected_source(doc);
        let id = doc.id().to_owned();
        let weak = cx.weak_entity();
        popover(
            "meeting-source-menu",
            Anchor::Slot(self.source_anchor.clone()),
        )
        .width(176.)
        .on_dismiss(move |_, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.source_open = false;
                cx.notify();
            });
        })
        .child(
            div()
                .id("meeting-audio-sources")
                .role(Role::Menu)
                .p_1()
                .children(
                    [
                        CaptureSource::MicrophoneAndSystem,
                        CaptureSource::Microphone,
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(ix, source)| {
                        let id = id.clone();
                        page::row(
                            ("meeting-source-option", ix),
                            audio_source_label(source),
                            selected == source,
                        )
                        .role(Role::MenuItemRadio)
                        .aria_selected(selected == source)
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .size(px(14.))
                                .when(selected == source, |d| d.child(icon("check", 14.))),
                        )
                        .child(page::label().child(audio_source_label(source)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.editable()
                                && this.document.as_ref().is_some_and(|doc| doc.id() == id)
                            {
                                this.source_overrides.insert(id.clone(), source);
                                this.source_open = false;
                                cx.notify();
                            }
                        }))
                    }),
                ),
        )
        .into_any_element()
    }
}

pub(super) fn editor(label: &'static str, input: &gpui::Entity<crate::input::Input>) -> AnyElement {
    div()
        .id(label)
        .role(Role::Region)
        .aria_label(label)
        .size_full()
        .min_h_0()
        .rounded(px(page::CARD_CORNER))
        .border_1()
        .border_color(page::divider())
        .overflow_hidden()
        .child(input.clone())
        .into_any_element()
}

pub(super) fn live_transcript(live: &Live) -> AnyElement {
    div()
        .id("meeting-live-transcript")
        .role(Role::Region)
        .aria_label("Live transcript")
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_3()
        .overflow_y_scroll()
        .track_scroll(&live.scroll)
        .when_some(
            live.prefix.as_ref().filter(|text| !text.is_empty()),
            |d, text| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .child(page::meta().child("Previous transcript"))
                        .child(page::body().child(text.clone())),
                )
            },
        )
        .when(
            live.prefix.is_none() && live.segments.is_empty() && live.partials.is_empty(),
            |d| {
                d.child(page::meta().child(if live.state.phase.active() {
                    "Waiting for transcript…"
                } else {
                    "Loading saved transcript…"
                }))
            },
        )
        .children(live.segments.iter().map(|segment| {
            div()
                .id(("meeting-live-segment", segment.sequence as usize))
                .flex_shrink_0()
                .child(page::meta().child(format!(
                    "{} · {}",
                    duration(segment.start_ms),
                    source(&segment.source)
                )))
                .child(page::body().child(segment.text.trim().to_owned()))
        }))
        .children(live.partials.iter().map(|(name, text)| {
            page::meta()
                .flex_shrink_0()
                .child(format!("{}: {text}", source(name)))
        }))
        .into_any_element()
}

fn source(name: &str) -> &str {
    if name == "microphone" {
        "Microphone"
    } else {
        "System audio"
    }
}

fn date(milliseconds: u64) -> String {
    chrono::DateTime::from_timestamp_millis(milliseconds as i64)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%b %-d, %-I:%M %p")
                .to_string()
        })
        .unwrap_or_default()
}

fn duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn audio_source_label(source: CaptureSource) -> &'static str {
    match source {
        CaptureSource::Microphone => "Mic only",
        CaptureSource::MicrophoneAndSystem => "Mic + system audio",
    }
}

pub(super) fn metadata(state: &State) -> String {
    if state.started_at == 0 && state.run.is_none() {
        return "Not recorded · 00:00:00".into();
    }
    let elapsed = duration(state.duration_ms());
    let phase = match state.phase {
        Phase::Starting => "Preparing · ",
        Phase::Recording => "Recording · ",
        Phase::Finishing => "Finishing · ",
        Phase::Stopped => "",
        Phase::Failed => "Failed · ",
        Phase::Interrupted => "Interrupted · ",
    };
    format!("{phase}{} · {elapsed}", date(state.started_at))
}
