//! Recording controls and the view's bounded live transcript.
use super::{
    Event, Page, capture,
    document::{Action, Document},
};
use gpui::{Context, ScrollHandle, px};
use std::{
    collections::{BTreeMap, VecDeque},
    time::Duration,
};
use vorkspace_meetings::recording::{self, Phase, Segment, State};

pub(super) struct Live {
    pub state: State,
    pub segments: VecDeque<Segment>,
    pub partials: BTreeMap<String, String>,
    pub scroll: ScrollHandle,
    pub prefix: Option<String>,
    base_sequence: u64,
    sequence: u64,
    bytes: usize,
    recover: bool,
    recovering: bool,
    content_announced: bool,
}

impl Live {
    fn new(state: State, recover: bool) -> Self {
        Self {
            state,
            segments: VecDeque::new(),
            partials: BTreeMap::new(),
            scroll: ScrollHandle::new(),
            prefix: None,
            base_sequence: 0,
            sequence: 0,
            bytes: 0,
            recover,
            recovering: false,
            content_announced: false,
        }
    }

    fn matches(&self, state: &State) -> bool {
        self.state.id == state.id && self.state.run == state.run
    }

    fn take_content_notification(&mut self, incoming: &[Segment]) -> bool {
        if self.content_announced {
            return false;
        }
        let meaningful = self
            .prefix
            .as_ref()
            .is_some_and(|text| !text.trim().is_empty())
            || self
                .segments
                .iter()
                .chain(incoming)
                .any(|segment| !segment.text.trim().is_empty());
        self.content_announced = meaningful;
        meaningful
    }

    pub(super) fn keep_corrected_prefix(&mut self, doc: &Document) {
        let Some(saved) = doc
            .saved
            .as_ref()
            .filter(|saved| saved.meeting.transcript_override.is_some())
        else {
            return;
        };
        self.base_sequence = saved.sequence;
        self.sequence = self.sequence.max(saved.sequence);
        while self
            .segments
            .front()
            .is_some_and(|segment| segment.sequence <= self.base_sequence)
        {
            self.bytes -= self.segments.pop_front().unwrap().text.len();
        }
        // Live capture is bounded. The whole corrected transcript remains in
        // the document and storage while the new take appends after it.
        let start = saved
            .transcript
            .ceil_char_boundary(saved.transcript.len().saturating_sub(24 * 1024));
        self.prefix = (!saved.transcript.is_empty()).then(|| saved.transcript[start..].to_owned());
    }

    fn anchor(&self) -> Option<u64> {
        let ix = self
            .scroll
            .top_item()
            .checked_sub(usize::from(self.prefix.is_some()))?;
        self.segments.get(ix).map(|segment| segment.sequence)
    }

    fn append(&mut self, segments: impl IntoIterator<Item = Segment>) {
        let follow = self.scroll.max_offset().y + self.scroll.offset().y < px(180.);
        let anchor = self.anchor();
        let first = self.segments.front().map(|s| s.sequence);
        for segment in segments {
            if segment.sequence <= self.sequence {
                continue;
            }
            if segment.sequence != self.sequence + 1 {
                self.recover = true;
                break;
            }
            self.push(segment);
        }
        if follow {
            self.scroll.scroll_to_bottom();
        } else if first != self.segments.front().map(|s| s.sequence) {
            let index = anchor.map_or(0, |anchor| {
                self.segments.partition_point(|s| s.sequence < anchor)
            });
            self.scroll
                .scroll_to_top_of_item(index + usize::from(self.prefix.is_some()));
        }
    }

    fn push(&mut self, segment: Segment) {
        while !self.segments.is_empty()
            && (self.segments.len() >= 400 || self.bytes + segment.text.len() > 512 * 1024)
        {
            self.bytes -= self.segments.pop_front().unwrap().text.len();
        }
        self.sequence = segment.sequence;
        self.bytes += segment.text.len();
        self.segments.push_back(segment);
    }

    fn apply_recovery(&mut self, segments: Vec<Segment>) {
        let follow = self.scroll.max_offset().y + self.scroll.offset().y < px(180.);
        let anchor = self.anchor();
        self.segments.clear();
        self.bytes = 0;
        self.sequence = self.base_sequence;
        let skip = segments
            .partition_point(|segment| segment.sequence <= self.base_sequence)
            .max(segments.len().saturating_sub(400));
        for segment in segments.into_iter().skip(skip) {
            self.push(segment);
        }
        self.recover = false;
        if follow {
            self.scroll.scroll_to_bottom();
        } else if let Some(anchor) = anchor {
            self.scroll.scroll_to_top_of_item(
                self.segments.partition_point(|s| s.sequence < anchor)
                    + usize::from(self.prefix.is_some()),
            );
        }
    }
}

impl Page {
    pub(super) fn observe_live(&mut self, state: State, cx: &mut Context<Self>) {
        if self.live.as_ref().is_none_or(|live| !live.matches(&state)) {
            let mut live = Live::new(state.clone(), true);
            if let Some(doc) = self
                .document
                .as_ref()
                .filter(|doc| doc.id() == state.id && doc.state.run != state.run)
            {
                live.keep_corrected_prefix(doc);
            }
            self.live = Some(live);
        }
        if let Some(doc) = self.document.as_mut().filter(|doc| doc.id() == state.id) {
            doc.state = state;
            doc.awaiting_transcript = false;
            self.sync_read_only(cx);
        }
        cx.notify();
    }

    pub(super) fn request_record(&mut self, id: &str, run: Option<&str>, cx: &mut Context<Self>) {
        if !self.enabled || self.preparing.is_some() || self.deleting {
            return;
        }
        let current = capture::current(cx);
        if let Some(run) = run {
            if current.is_some_and(|state| {
                state.id == id
                    && state.run.as_deref() == Some(run)
                    && state.phase != Phase::Finishing
            }) {
                match capture::stop(id, run, cx) {
                    Ok(()) => self.poll(cx),
                    Err(error) => self.fail(error, cx),
                }
            } else {
                self.poll(cx);
            }
        } else if current.is_some() {
            // A Start click cannot become a Stop if a different window starts
            // recording before the click reaches this view.
            self.poll(cx);
        } else if self.editable() && self.document.as_ref().is_some_and(|doc| doc.id() == id) {
            self.navigate(Action::Record(id.to_owned()), cx);
        }
    }

    pub(super) fn start_recording(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.preparing.is_some() || capture::active(cx) || !self.editable() {
            return;
        }
        let Some(doc) = self.document.as_ref().filter(|doc| doc.id() == id) else {
            return;
        };
        let Some(lease) = doc.lease.clone() else {
            return;
        };
        let previous_run = doc.state.run.clone();
        let source = self.selected_source(doc);
        let Some(meetings) = self.storage(cx) else {
            return;
        };
        let id = id.to_owned();
        if let Err(error) = self.attach_note(&id, cx) {
            self.fail(error, cx);
            return;
        }
        self.preparing = Some(id.clone());
        self.error = None;
        self.sync_read_only(cx);
        let enable_epoch = self.enable_epoch;
        let intent = self.intent;
        let directory = meetings.directory.clone();
        let prepare_id = id.clone();
        let worker_lease = lease.clone();
        let work = cx
            .background_executor()
            .spawn(async move { worker_lease.materialize(&meetings, &prepare_id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let _lease = lease;
                this.preparing = None;
                if this.enable_epoch == enable_epoch {
                    match result.and_then(|()| {
                        capture::start(id.clone(), directory, previous_run, source, cx)
                    }) {
                        Ok(mut state) => {
                            let previous = this.document.as_ref().filter(|doc| doc.id() == id);
                            let recover = previous.is_none_or(|doc| !doc.unrecorded());
                            if let Some(doc) = previous.filter(|doc| !doc.unrecorded()) {
                                state.started_at = doc.state.started_at;
                                state.offset_ms = doc.state.duration_ms();
                            }
                            let mut live = Live::new(state.clone(), recover);
                            if let Some(doc) = previous {
                                live.keep_corrected_prefix(doc);
                            }
                            this.live = Some(live);
                            if state.phase.active() {
                                this.observe_live(state, cx);
                            } else {
                                this.finish_live(state, cx);
                            }
                            if this.intent == intent
                                && this.document.as_ref().is_some_and(|doc| doc.id() == id)
                            {
                                this.section = super::document::Section::Transcript;
                                this.source_open = false;
                            }
                        }
                        Err(error) => this.fail(error, cx),
                    }
                }
                this.sync_read_only(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn start_polling(&mut self, cx: &mut Context<Self>) {
        self.poll(cx);
        self.poller = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if !self.enabled || !self.active {
            return;
        }
        let current = capture::current(cx);
        let was_active = self
            .live
            .as_ref()
            .is_some_and(|live| live.state.phase.active());
        if !was_active {
            let Some(state) = current.clone() else {
                return;
            };
            self.observe_live(state, cx);
        } else if let Some(state) = current.as_ref()
            && self
                .live
                .as_ref()
                .is_some_and(|live| live.state.id == state.id && !live.matches(state))
        {
            self.observe_live(state.clone(), cx);
        }
        let Ok(meetings) = self.meetings.clone() else {
            return;
        };
        let live = self.live.as_mut().unwrap();
        let id = live.state.id.clone();
        let result = match capture::status(&id, live.sequence, meetings.directory.clone(), cx) {
            Ok(result) => result,
            Err(error) => {
                if current.is_none_or(|state| !live.matches(&state)) {
                    let state = State {
                        phase: Phase::Interrupted,
                        error: Some(error),
                        ..live.state.clone()
                    };
                    self.finish_live(state, cx);
                } else {
                    self.fail(error, cx);
                }
                return;
            }
        };
        if !live.matches(&result.state) {
            // A prior durable take cannot stop a newer take which the capture
            // owner still holds. If the owner has moved to another meeting,
            // settle this one from its newest available terminal snapshot.
            if !result.state.phase.active() && current.is_none_or(|state| state.id != id) {
                self.finish_live(result.state, cx);
            }
            return;
        }
        if !result.state.phase.active() {
            self.finish_live(result.state, cx);
            return;
        }
        let phase_changed = live.state.phase != result.state.phase;
        let mut state = result.state;
        if state.phase == Phase::Starting && state.offset_ms < live.state.offset_ms {
            state.started_at = live.state.started_at;
            state.offset_ms = live.state.offset_ms;
        }
        live.state = state;
        live.partials = result.partials;
        live.recover |= result.reload;
        let first_content = live.take_content_notification(&result.segments);
        if live.recover {
            if !live.recovering && live.state.phase != Phase::Starting {
                live.recovering = true;
                let run = live.state.run.clone();
                self.recover_live(id.clone(), run, meetings.directory, cx);
            }
        } else {
            live.append(result.segments);
        }
        if first_content || phase_changed {
            // Notes owns collection loading. Publish only lifecycle/content
            // boundaries, rather than scanning all recordings on every poll.
            cx.emit(Event::Changed);
        }
        if !was_active || phase_changed || self.document.as_ref().is_some_and(|doc| doc.id() == id)
        {
            cx.notify();
        }
    }

    fn recover_live(
        &mut self,
        id: String,
        run: Option<String>,
        directory: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let recover_id = id.clone();
        let work = cx
            .background_executor()
            .spawn(async move { recording::read(&directory, &recover_id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let Some(live) = this.live.as_mut().filter(|live| {
                    live.state.id == id && live.state.run == run && live.state.phase.active()
                }) else {
                    return;
                };
                live.recovering = false;
                match result {
                    Ok(transcript) if live.matches(&transcript.state) => {
                        if transcript.state.phase.active() {
                            live.apply_recovery(transcript.segments);
                        } else {
                            // Poll the capture owner to adopt the durable
                            // terminal state before updating the view.
                            this.poll(cx);
                        }
                    }
                    Ok(_) => {}
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn finish_live(&mut self, state: State, cx: &mut Context<Self>) {
        let id = state.id.clone();
        let live = self.live.as_mut().unwrap();
        live.state = state.clone();
        live.partials.clear();
        if let Some(doc) = self.document.as_mut().filter(|doc| doc.id() == id) {
            doc.state = state.clone();
            doc.awaiting_transcript = true;
            self.sync_read_only(cx);
            // Let a deliberate selection already in flight finish before
            // replacing the finished recording's retained transcript.
            if !self.opening {
                self.open(id.clone(), cx);
            }
        }
        if let Some(error) = state.error {
            self.fail(error, cx);
        }
        // Completion must refresh Notes even when a different note is selected.
        cx.emit(Event::Changed);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gap_is_recovered_by_sequence_and_the_live_window_stays_bounded() {
        let mut live = Live::new(
            State {
                id: "meeting".into(),
                started_at: 0,
                ended_at: None,
                phase: Phase::Recording,
                ..State::starting(
                    "meeting".into(),
                    "run-one".into(),
                    recording::CaptureSource::default(),
                )
            },
            false,
        );
        let segment = |sequence| Segment {
            sequence,
            source: "microphone".into(),
            start_ms: 0,
            end_ms: 10,
            text: "segment".into(),
        };
        live.append([segment(1), segment(1), segment(3)]);
        assert_eq!(live.sequence, 1);
        assert!(live.recover);
        live.apply_recovery((1..=500).map(segment).collect());
        assert!(!live.recover);
        assert_eq!(live.sequence, 500);
        assert_eq!(live.segments.len(), 400);
        assert_eq!(live.segments.front().unwrap().sequence, 101);
        live.append([segment(500), segment(501)]);
        assert_eq!(live.sequence, 501);
        assert_eq!(live.segments.front().unwrap().sequence, 102);
    }

    #[test]
    fn first_meaningful_content_notifies_once_without_whitespace_consuming_the_notice() {
        let mut live = Live::new(State::dormant("recording".into()), false);
        let segment = |sequence, text: &str| Segment {
            sequence,
            source: "microphone".into(),
            start_ms: 0,
            end_ms: 10,
            text: text.into(),
        };
        let whitespace = segment(1, " \n");
        assert!(!live.take_content_notification(std::slice::from_ref(&whitespace)));
        live.append([whitespace]);
        assert!(!live.take_content_notification(&[]));
        let first_words = segment(2, "Taylor owns the release.");
        assert!(live.take_content_notification(std::slice::from_ref(&first_words)));
        live.append([first_words]);
        assert!(!live.take_content_notification(&[segment(3, "New words")]));
    }
}
