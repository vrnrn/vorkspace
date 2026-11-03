//! Meetings owns capture admission, receipts, and the bounded live transcript.
use crate::background_helper::{self, CaptureEvent};
use gpui::{App, AppContext, Context, Entity, Global, Subscription};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    time::Duration,
};
use vorkspace_helper::{AudioNotice, AudioRequest, Command};
use vorkspace_meetings::recording::{self, Phase, Segment, State};

const OWNER: &str = "meetings";

#[derive(Default)]
struct Recording {
    pending: Option<AudioRequest>,
    state: Option<State>,
    recent: VecDeque<Segment>,
    bytes: usize,
    partials: BTreeMap<String, String>,
}

impl Recording {
    fn active(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.phase.active())
    }

    fn matches(&self, id: &str) -> bool {
        self.state.as_ref().is_some_and(|state| state.id == id)
    }
}

struct Capture {
    helper: Entity<background_helper::Service>,
    _subscription: Subscription,
    recording: Recording,
}

struct MeetingCapture(Entity<Capture>);
impl Global for MeetingCapture {}

fn service(cx: &mut App) -> Entity<Capture> {
    if let Some(capture) = cx.try_global::<MeetingCapture>() {
        return capture.0.clone();
    }
    let helper = background_helper::ensure(cx);
    let capture = cx.new(|cx: &mut Context<Capture>| Capture {
        _subscription: cx.subscribe(&helper, |capture, _, event, cx| match event {
            CaptureEvent::Connected => capture.flush_start(cx),
            CaptureEvent::Audio(notice) => capture.receive(notice.clone(), cx),
        }),
        helper,
        recording: Recording::default(),
    });
    cx.set_global(MeetingCapture(capture.clone()));
    capture
}

pub(crate) fn init(cx: &mut App) {
    service(cx);
}

pub(super) fn current(cx: &App) -> Option<State> {
    let recording = &cx.try_global::<MeetingCapture>()?.0.read(cx).recording;
    recording
        .state
        .as_ref()
        .filter(|state| state.phase.active())
        .cloned()
}

pub(crate) fn stop_active(cx: &mut App) -> Result<(), String> {
    if let Some(state) = current(cx) {
        stop(&state.id, cx)?;
    }
    Ok(())
}

pub(crate) fn active(cx: &App) -> bool {
    cx.try_global::<MeetingCapture>()
        .is_some_and(|capture| capture.0.read(cx).recording.active())
}

impl Capture {
    fn sync_helper(&self, cx: &mut Context<Self>) {
        self.helper.update(cx, |helper, cx| {
            helper.set_capture_active(self.recording.active(), cx);
        });
    }

    fn flush_start(&mut self, cx: &mut Context<Self>) {
        if self.helper.read(cx).connected()
            && let Some(request) = self.recording.pending.take()
            && !self.helper.read(cx).send(Command::StartAudio(request))
            && let Some(state) = &mut self.recording.state
        {
            state.phase = Phase::Interrupted;
            state.error = Some("The helper disconnected while starting. Check the saved recording before trying again.".into());
            self.sync_helper(cx);
        }
    }

    fn receive(&mut self, notice: AudioNotice, cx: &mut Context<Self>) {
        let recording = &mut self.recording;
        match notice {
            AudioNotice::Idle => {
                // Initial Idle can arrive after Start was sent. Starting remains
                // uncertain until its receipt or the admission deadline.
                if let Some(state) = &mut recording.state
                    && recording.pending.is_none()
                    && state.phase.active()
                    && state.phase != Phase::Starting
                {
                    state.phase = Phase::Interrupted;
                    state.error = Some(
                        "The recording helper was interrupted. Saved transcript text is preserved."
                            .into(),
                    );
                }
            }
            AudioNotice::State { owner, state }
                if owner == OWNER
                    && (recording.state.is_none() || recording.matches(&state.id)) =>
            {
                let finishing = recording
                    .state
                    .as_ref()
                    .is_some_and(|current| current.phase == Phase::Finishing);
                if !finishing || !state.phase.active() {
                    recording.state = Some(state);
                }
            }
            AudioNotice::Segment { id, owner, segment }
                if owner == OWNER && recording.matches(&id) =>
            {
                recording.partials.remove(&segment.source);
                while !recording.recent.is_empty()
                    && (recording.recent.len() >= 256
                        || recording.bytes + segment.text.len() > 512 * 1024)
                {
                    recording.bytes -= recording.recent.pop_front().unwrap().text.len();
                }
                recording.bytes += segment.text.len();
                recording.recent.push_back(segment);
                return;
            }
            AudioNotice::Partial {
                id,
                owner,
                source,
                text,
            } if owner == OWNER && recording.matches(&id) => {
                recording.partials.insert(source, text);
                return;
            }
            _ => return,
        }
        self.sync_helper(cx);
    }

    fn expire_start(&mut self, id: &str, directory: &Path, cx: &mut Context<Self>) {
        let awaiting_connection = self
            .recording
            .pending
            .as_ref()
            .is_some_and(|request| request.id == id);
        let uncertain = self
            .recording
            .state
            .as_ref()
            .is_some_and(|state| state.id == id && state.phase == Phase::Starting)
            && recording::state(directory, id).is_err();
        if !awaiting_connection && !uncertain {
            return;
        }
        if !awaiting_connection {
            self.helper.read(cx).send(finish_command(id));
        }
        self.recording.pending = None;
        if let Some(state) = &mut self.recording.state {
            state.phase = Phase::Failed;
            state.error = Some("The helper did not confirm this recording. Saved text is preserved; check the meeting before starting again.".into());
        }
        self.sync_helper(cx);
    }
}

fn finish_command(id: &str) -> Command {
    Command::FinishAudio {
        id: id.into(),
        owner: OWNER.into(),
    }
}

pub(super) fn start(id: String, directory: PathBuf, cx: &mut App) -> Result<Value, String> {
    if !vorkspace_meetings::valid_id(&id) {
        return Err("Invalid recording id".into());
    }
    service(cx).update(cx, |capture, cx| {
        let helper = capture.helper.read(cx);
        if helper.dictation_status.active || capture.recording.active() {
            return Err("Finish the current voice activity before recording a meeting.".into());
        }
        let dictation = helper.dictation_config();
        capture.recording = Recording {
            pending: Some(AudioRequest {
                id: id.clone(),
                owner: OWNER.into(),
                data_dir: directory.clone(),
                microphone: dictation.microphone.clone(),
                words: dictation.words.clone(),
            }),
            state: Some(State {
                id: id.clone(),
                started_at: recording::now_ms(),
                ended_at: None,
                phase: Phase::Starting,
                error: None,
            }),
            ..Default::default()
        };
        capture.sync_helper(cx);
        capture.flush_start(cx);
        let pending_id = id.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let _ = this.update(cx, |capture, cx| {
                capture.expire_start(&pending_id, &directory, cx)
            });
        })
        .detach();
        Ok(json!({"id":id,"state":capture.recording.state}))
    })
}

pub(super) fn stop(id: &str, cx: &mut App) -> Result<Value, String> {
    if !vorkspace_meetings::valid_id(id) {
        return Err("Invalid recording id".into());
    }
    service(cx).update(cx, |capture, cx| {
        let recording = &mut capture.recording;
        let current = recording.matches(id);
        if current && !recording.active() {
            return Ok(json!({"accepted":true}));
        }
        if current && recording.pending.take().is_some() {
            if let Some(state) = &mut recording.state {
                state.phase = Phase::Stopped;
                state.ended_at = Some(recording::now_ms());
            }
            capture.sync_helper(cx);
        } else {
            // Native ownership validates the recording ID, including after reconnect.
            if !capture.helper.read(cx).send(finish_command(id)) {
                return Err("Recording stop could not be delivered. Saved text is preserved; check the meeting before starting again.".into());
            }
            if current && let Some(state) = &mut recording.state {
                state.phase = Phase::Finishing;
            }
        }
        Ok(json!({"accepted":true}))
    })
}

pub(super) fn status(
    id: &str,
    after: u64,
    directory: PathBuf,
    cx: &mut App,
) -> Result<Value, String> {
    if !vorkspace_meetings::valid_id(id) {
        return Err("Invalid recording id".into());
    }
    service(cx).update(cx, |capture, cx| {
        if !capture.recording.matches(id) {
            let state = recording::state(&directory, id)?;
            return Ok(json!({"state":state,"segments":[],"partials":{},"reload":true}));
        }
        // Durable metadata can recover a missed terminal receipt without reading text.
        if capture.recording.active()
            && let Ok(state) = recording::state(&directory, id)
            && !state.phase.active()
        {
            capture.recording.state = Some(state);
            capture.recording.partials.clear();
            capture.sync_helper(cx);
        }
        let recording = &capture.recording;
        let next = after.saturating_add(1);
        let first = recording.recent.front().map_or(next, |segment| segment.sequence);
        let segments = recording.recent.iter().filter(|segment| segment.sequence > after).collect::<Vec<_>>();
        Ok(json!({"state":recording.state,"segments":segments,"partials":recording.partials,"reload":next<first}))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn helper_receipts_distinguish_starting_from_an_interrupted_recording(
        cx: &mut gpui::TestAppContext,
    ) {
        let capture = cx.update(service);
        let helper = capture.update(cx, |capture, cx| {
            capture.recording.state = Some(State {
                id: "sample".into(),
                started_at: 0,
                ended_at: None,
                phase: Phase::Starting,
                error: None,
            });
            capture.sync_helper(cx);
            capture.helper.clone()
        });
        let emit = |notice, cx: &mut gpui::TestAppContext| {
            helper.update(cx, |_, cx| cx.emit(CaptureEvent::Audio(notice)));
        };
        let phase = |cx: &mut gpui::TestAppContext| {
            capture.read_with(cx, |capture, _| {
                capture.recording.state.as_ref().unwrap().phase.clone()
            })
        };
        emit(AudioNotice::Idle, cx);
        assert_eq!(phase(cx), Phase::Starting);
        emit(
            AudioNotice::State {
                owner: OWNER.into(),
                state: State {
                    id: "sample".into(),
                    started_at: 0,
                    ended_at: None,
                    phase: Phase::Recording,
                    error: None,
                },
            },
            cx,
        );
        assert_eq!(phase(cx), Phase::Recording);
        emit(AudioNotice::Idle, cx);
        assert_eq!(phase(cx), Phase::Interrupted);
        assert!(!cx.update(|cx| active(cx)));
    }
}
