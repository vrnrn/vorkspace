//! Meetings owns capture admission, receipts, and the bounded live transcript.
use crate::background_helper::{self, CaptureEvent};
use gpui::{App, AppContext, Context, Entity, Global, Subscription};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    time::Duration,
};
use vorkspace_helper::{AudioNotice, AudioRequest, Command};
use vorkspace_meetings::recording::{self, CaptureSource, Phase, Segment, State};

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

    fn matches_run(&self, id: &str, run: &str) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.id == id && state.run.as_deref() == Some(run))
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

pub(crate) fn current(cx: &App) -> Option<State> {
    let recording = &cx.try_global::<MeetingCapture>()?.0.read(cx).recording;
    recording
        .state
        .as_ref()
        .filter(|state| state.phase.active())
        .cloned()
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
                    && (recording.state.is_none()
                        || state
                            .run
                            .as_deref()
                            .is_some_and(|run| recording.matches_run(&state.id, run))
                        || (!recording.active() && state.phase.active())) =>
            {
                let same_run = recording
                    .state
                    .as_ref()
                    .is_some_and(|current| current.id == state.id && current.run == state.run);
                let finishing = recording
                    .state
                    .as_ref()
                    .is_some_and(|current| same_run && current.phase == Phase::Finishing);
                if !finishing || !state.phase.active() {
                    if !same_run {
                        recording.recent.clear();
                        recording.bytes = 0;
                        recording.partials.clear();
                    }
                    recording.state = Some(state);
                }
            }
            AudioNotice::Segment {
                id,
                run,
                owner,
                segment,
            } if owner == OWNER && recording.matches_run(&id, &run) => {
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
                run,
                owner,
                source,
                text,
            } if owner == OWNER && recording.matches_run(&id, &run) => {
                recording.partials.insert(source, text);
                return;
            }
            _ => return,
        }
        self.sync_helper(cx);
    }

    fn expire_start(&mut self, id: &str, run: &str, directory: &Path, cx: &mut Context<Self>) {
        if !self.recording.matches_run(id, run) {
            return;
        }
        let awaiting_connection = self
            .recording
            .pending
            .as_ref()
            .is_some_and(|request| request.id == id && request.run == run);
        let uncertain = self
            .recording
            .state
            .as_ref()
            .is_some_and(|state| state.id == id && state.phase == Phase::Starting)
            && !recording::state(directory, id)
                .is_ok_and(|state| state.run.as_deref() == Some(run));
        if !awaiting_connection && !uncertain {
            return;
        }
        if !awaiting_connection {
            self.helper.read(cx).send(finish_command(id, run));
        }
        self.recording.pending = None;
        if let Some(state) = &mut self.recording.state {
            state.phase = Phase::Failed;
            state.error = Some("The helper did not confirm this recording. Saved text is preserved; check the meeting before starting again.".into());
        }
        self.sync_helper(cx);
    }
}

fn finish_command(id: &str, run: &str) -> Command {
    Command::FinishAudio {
        id: id.into(),
        owner: OWNER.into(),
        run: run.into(),
    }
}

pub(super) fn start(
    id: String,
    directory: PathBuf,
    previous_run: Option<String>,
    source: CaptureSource,
    cx: &mut App,
) -> Result<State, String> {
    if !vorkspace_meetings::valid_id(&id) {
        return Err("Invalid recording id".into());
    }
    service(cx).update(cx, |capture, cx| {
        let helper = capture.helper.read(cx);
        if helper.dictation_status.active || capture.recording.active() {
            return Err("Finish the current voice activity before recording a meeting.".into());
        }
        let dictation = helper.dictation_config();
        let run = uuid::Uuid::new_v4().simple().to_string();
        capture.recording = Recording {
            pending: Some(AudioRequest {
                id: id.clone(),
                run: run.clone(),
                previous_run,
                source,
                owner: OWNER.into(),
                data_dir: directory.clone(),
                microphone: dictation.microphone.clone(),
                words: dictation.words.clone(),
            }),
            state: Some(State::starting(id.clone(), run.clone(), source)),
            ..Default::default()
        };
        capture.sync_helper(cx);
        capture.flush_start(cx);
        let pending_id = id.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let _ = this.update(cx, |capture, cx| {
                capture.expire_start(&pending_id, &run, &directory, cx)
            });
        })
        .detach();
        Ok(capture.recording.state.clone().unwrap())
    })
}

pub(super) fn stop(id: &str, run: &str, cx: &mut App) -> Result<(), String> {
    if !vorkspace_meetings::valid_id(id) || !vorkspace_meetings::valid_id(run) {
        return Err("Invalid recording id".into());
    }
    service(cx).update(cx, |capture, cx| {
        let recording = &mut capture.recording;
        let current = recording.matches_run(id, run);
        if recording.matches(id) && !current {
            return Ok(());
        }
        if current && !recording.active() {
            return Ok(());
        }
        if current && recording.pending.take().is_some() {
            if let Some(state) = &mut recording.state {
                state.phase = Phase::Stopped;
                state.ended_at = Some(recording::now_ms());
            }
            capture.sync_helper(cx);
        } else {
            // Native ownership validates the recording ID, including after reconnect.
            if !capture.helper.read(cx).send(finish_command(id, run)) {
                return Err("Recording stop could not be delivered. Saved text is preserved; check the meeting before starting again.".into());
            }
            if current && let Some(state) = &mut recording.state {
                state.phase = Phase::Finishing;
            }
        }
        Ok(())
    })
}

pub(super) struct Feed {
    pub state: State,
    pub segments: Vec<Segment>,
    pub partials: BTreeMap<String, String>,
    pub reload: bool,
}

pub(super) fn status(
    id: &str,
    after: u64,
    directory: PathBuf,
    cx: &mut App,
) -> Result<Feed, String> {
    if !vorkspace_meetings::valid_id(id) {
        return Err("Invalid recording id".into());
    }
    service(cx).update(cx, |capture, cx| {
        if !capture.recording.matches(id) {
            let state = recording::state(&directory, id)?;
            return Ok(Feed {
                state,
                segments: Vec::new(),
                partials: BTreeMap::new(),
                reload: true,
            });
        }
        // Durable metadata can recover a missed terminal receipt without reading text.
        if capture.recording.active()
            && let Ok(state) = recording::state(&directory, id)
            && state.run == capture.recording.state.as_ref().unwrap().run
            && !state.phase.active()
        {
            capture.recording.state = Some(state);
            capture.recording.partials.clear();
            capture.sync_helper(cx);
        }
        let recording = &capture.recording;
        let next = after.saturating_add(1);
        let first = recording
            .recent
            .front()
            .map_or(next, |segment| segment.sequence);
        let segments = recording
            .recent
            .iter()
            .filter(|segment| segment.sequence > after)
            .cloned()
            .collect();
        Ok(Feed {
            state: recording.state.clone().unwrap(),
            segments,
            partials: recording.partials.clone(),
            reload: next < first,
        })
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
            capture.recording.state = Some(State::starting(
                "sample".into(),
                "run-one".into(),
                CaptureSource::Microphone,
            ));
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
                    phase: Phase::Recording,
                    ..State::starting("sample".into(), "run-one".into(), CaptureSource::Microphone)
                },
            },
            cx,
        );
        assert_eq!(phase(cx), Phase::Recording);
        emit(AudioNotice::Idle, cx);
        assert_eq!(phase(cx), Phase::Interrupted);
        assert!(!cx.update(|cx| active(cx)));
    }

    #[gpui::test]
    fn a_previous_take_cannot_finish_or_append_to_a_restarted_meeting(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let mut owner = recording::Owner::create(directory.path(), "sample").unwrap();
        owner.phase(Phase::Stopped, None).unwrap();
        drop(owner);
        let capture = cx.update(service);
        capture.update(cx, |capture, _| {
            capture.recording.state = Some(State::starting(
                "sample".into(),
                "second".into(),
                CaptureSource::Microphone,
            ));
        });
        let feed = cx
            .update(|cx| status("sample", 0, directory.path().to_owned(), cx))
            .unwrap();
        assert_eq!(
            feed.state.phase,
            Phase::Starting,
            "prior durable Stop is not this take's receipt"
        );
        capture.update(cx, |capture, cx| {
            capture.expire_start("sample", "first", directory.path(), cx);
            capture.receive(
                AudioNotice::State {
                    owner: OWNER.into(),
                    state: State {
                        phase: Phase::Stopped,
                        ..State::starting(
                            "sample".into(),
                            "first".into(),
                            CaptureSource::Microphone,
                        )
                    },
                },
                cx,
            );
            capture.receive(
                AudioNotice::Partial {
                    id: "sample".into(),
                    run: "first".into(),
                    owner: OWNER.into(),
                    source: "microphone".into(),
                    text: "stale speech".into(),
                },
                cx,
            );
            assert!(capture.recording.partials.is_empty());
            assert_eq!(
                capture.recording.state.as_ref().unwrap().phase,
                Phase::Starting
            );
        });
        cx.update(|cx| stop("sample", "first", cx)).unwrap();
        let state = cx.update(|cx| current(cx)).unwrap();
        assert_eq!(state.run.as_deref(), Some("second"));
        assert_eq!(state.phase, Phase::Starting);
    }
}
