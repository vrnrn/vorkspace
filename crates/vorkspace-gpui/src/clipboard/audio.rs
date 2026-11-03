//! App-side receipts and a bounded live delta window; durable text stays in the recorder.
use super::*;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};
use vorkspace_helper::{AudioNotice, AudioRequest};
use vorkspace_meetings::recording::{self, Phase, Segment, State};
#[derive(Default)]
pub(super) struct Audio {
    pub wanted: bool,
    pub pending: Option<AudioRequest>,
    owner: String,
    state: Option<State>,
    recent: VecDeque<Segment>,
    bytes: usize,
    partials: BTreeMap<String, String>,
}
impl Service {
    pub(super) fn flush_audio(&mut self) {
        #[cfg(not(test))]
        if self.supervisor.as_ref().is_some_and(Supervisor::connected)
            && let Some(request) = self.audio.pending.take()
            && !self.send(Command::StartAudio(request))
            && let Some(state) = &mut self.audio.state
        {
            self.audio.wanted = false;
            state.phase = Phase::Interrupted;
            state.error=Some("The helper disconnected while starting. Check the saved recording before trying again.".into());
        }
    }
    pub(super) fn audio_notice(&mut self, notice: AudioNotice, cx: &mut Context<Self>) {
        match notice {
            AudioNotice::Idle => {
                // The connection's initial Idle can arrive after a queued Start
                // was sent. Starting remains uncertain until its receipt or the
                // admission deadline, never falsely interrupted by that snapshot.
                if self.audio.pending.is_none()
                    && self
                        .audio
                        .state
                        .as_ref()
                        .is_some_and(|s| s.phase.active() && s.phase != Phase::Starting)
                {
                    self.audio.state.as_mut().unwrap().phase = Phase::Interrupted;
                    self.audio.state.as_mut().unwrap().error = Some(
                        "The recording helper was interrupted. Saved transcript text is preserved."
                            .into(),
                    );
                    self.audio.wanted = false;
                }
            }
            AudioNotice::State { owner, state } => {
                if self.audio.state.as_ref().is_some_and(|s| s.id != state.id) {
                    return;
                }
                self.audio.owner = owner;
                self.audio.wanted = state.phase.active();
                self.audio.state = Some(state);
            }
            AudioNotice::Segment { id, owner, segment } => {
                if owner != self.audio.owner
                    || !self.audio.state.as_ref().is_some_and(|s| s.id == id)
                {
                    return;
                }
                self.audio.partials.remove(&segment.source);
                while !self.audio.recent.is_empty()
                    && (self.audio.recent.len() >= 256
                        || self.audio.bytes + segment.text.len() > 512 * 1024)
                {
                    self.audio.bytes -= self.audio.recent.pop_front().unwrap().text.len();
                }
                self.audio.bytes += segment.text.len();
                self.audio.recent.push_back(segment);
            }
            AudioNotice::Partial {
                id,
                owner,
                source,
                text,
            } => {
                if owner == self.audio.owner
                    && self.audio.state.as_ref().is_some_and(|s| s.id == id)
                {
                    self.audio.partials.insert(source, text);
                }
            }
        }
        #[cfg(not(test))]
        if let Some(supervisor) = &self.supervisor {
            supervisor.keep_alive(self.audio.wanted);
        }
        #[cfg(not(test))]
        self.sync_status(cx);
        cx.notify();
    }
}
pub(crate) fn owned(owner: &str, cx: &App) -> Option<String> {
    get(cx).and_then(|service| {
        let service = service.read(cx);
        (service.audio.owner == owner)
            .then(|| {
                service
                    .audio
                    .state
                    .as_ref()
                    .filter(|state| state.phase.active())
                    .map(|s| s.id.clone())
            })
            .flatten()
    })
}
pub(crate) fn start(
    owner: String,
    id: String,
    directory: PathBuf,
    cx: &mut App,
) -> Result<Value, String> {
    if !vorkspace_meetings::valid_id(&owner) || !vorkspace_meetings::valid_id(&id) {
        return Err("Invalid recording identity".into());
    }
    service(cx).update(cx, |s, cx| {
        if s.dictation_status.active || s.audio.state.as_ref().is_some_and(|s| s.phase.active()) {
            return Err("Finish the current voice activity before recording a meeting.".into());
        }
        let request = AudioRequest {
            id: id.clone(),
            owner: owner.clone(),
            data_dir: directory.clone(),
            microphone: s.dictation.microphone.clone(),
            words: s.dictation.words.clone(),
        };
        s.audio = Audio {
            wanted: true,
            pending: Some(request),
            owner,
            state: Some(State {
                id: id.clone(),
                started_at: recording::now_ms(),
                ended_at: None,
                phase: Phase::Starting,
                error: None,
            }),
            ..Default::default()
        };
        #[cfg(not(test))]
        if let Some(supervisor) = &s.supervisor {
            supervisor.keep_alive(true);
        }
        #[cfg(not(test))]
        s.sync_status(cx);
        s.flush_audio();
        let pending_id = id.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            let _ = this.update(cx, |s, cx| {
                let awaiting_connection=s.audio.pending.as_ref().is_some_and(|r|r.id==pending_id);
                let uncertain_start=s.audio.state.as_ref().is_some_and(|state|state.id==pending_id && state.phase==Phase::Starting) && recording::state(&directory,&pending_id).is_err();
                if awaiting_connection || uncertain_start {
                    if !awaiting_connection { s.send(Command::FinishAudio{id:pending_id.clone(),owner:s.audio.owner.clone()}); }
                    s.audio.pending = None;
                    s.audio.wanted = false;
                    if let Some(state) = &mut s.audio.state {
                        state.phase = Phase::Failed;
                        state.error =
                            Some("The helper did not confirm this recording. Saved text is preserved; check the meeting before starting again.".into());
                    }
                    #[cfg(not(test))]
                    if let Some(supervisor) = &s.supervisor {
                        supervisor.keep_alive(false);
                    }
                    #[cfg(not(test))]
                    s.sync_status(cx);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
        Ok(json!({"id":id,"accepted":true}))
    })
}
pub(crate) fn stop(owner: &str, id: &str, cx: &mut App) -> Result<Value, String> {
    if !vorkspace_meetings::valid_id(id) {
        return Err("Invalid recording id".into());
    }
    service(cx).update(cx, |s, cx| {
        let current = s.audio.owner == owner && s.audio.state.as_ref().is_some_and(|state|state.id == id);
        if current && s.audio.state.as_ref().is_some_and(|state|!state.phase.active()) {
            return Ok(json!({"accepted":true}));
        }
        if current && s.audio.pending.take().is_some() {
            s.audio.wanted = false;
            if let Some(state) = &mut s.audio.state {
                state.phase = Phase::Stopped;
                state.ended_at = Some(recording::now_ms());
            }
            #[cfg(not(test))]
            if let Some(supervisor) = &s.supervisor { supervisor.keep_alive(false); }
        } else {
            // The native owner validates plugin + recording ID. This also works
            // after reconnect when the app has no matching cached receipt yet.
            if !s.send(Command::FinishAudio {id:id.into(),owner:owner.into()}) {
                return Err("Recording stop could not be delivered. Saved text is preserved; check the meeting before starting again.".into());
            }
            if current && let Some(state) = &mut s.audio.state { state.phase=Phase::Finishing; }
        }
        #[cfg(not(test))]
        s.sync_status(cx);
        cx.notify();
        Ok(json!({"accepted":true}))
    })
}
pub(crate) fn status(
    owner: &str,
    id: &str,
    after: u64,
    directory: PathBuf,
    cx: &mut App,
) -> Result<Value, String> {
    if !vorkspace_meetings::valid_id(id) {
        return Err("Invalid recording id".into());
    }
    service(cx).update(cx, |s, cx| {
        if s.audio.owner != owner || !s.audio.state.as_ref().is_some_and(|state|state.id == id) {
            let state = recording::state(&directory,id)?;
            return Ok(json!({"state":state,"segments":[],"partials":{},"reload":true}));
        }
        // Metadata is authoritative even if the final IPC notice was dropped.
        // This never loads or rewrites transcript text.
        if s.audio.state.as_ref().is_some_and(|state|state.phase.active())
            && let Ok(state) = recording::state(&directory,id)
            && !state.phase.active() {
                s.audio.state = Some(state);
                s.audio.partials.clear();
                s.audio.wanted = false;
                #[cfg(not(test))]
                if let Some(supervisor) = &s.supervisor { supervisor.keep_alive(false); }
                #[cfg(not(test))]
                s.sync_status(cx);
                cx.notify();
        }
        let next = after.saturating_add(1);
        let first = s.audio.recent.front().map_or(next,|segment|segment.sequence);
        let segments = s.audio.recent.iter().filter(|segment|segment.sequence>after).collect::<Vec<_>>();
        Ok(json!({"state":s.audio.state,"segments":segments,"partials":s.audio.partials,"reload":next<first}))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn connection_idle_cannot_interrupt_an_unacknowledged_start(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let service = service(cx);
            service.update(cx, |s, cx| {
                s.audio.owner = "meetings".into();
                s.audio.wanted = true;
                s.audio.state = Some(State {
                    id: "sample".into(),
                    started_at: 0,
                    ended_at: None,
                    phase: Phase::Starting,
                    error: None,
                });
                s.audio_notice(AudioNotice::Idle, cx);
                assert_eq!(s.audio.state.as_ref().unwrap().phase, Phase::Starting);
                assert!(s.audio.wanted);
                s.audio.state.as_mut().unwrap().phase = Phase::Recording;
                s.audio_notice(AudioNotice::Idle, cx);
                assert_eq!(s.audio.state.as_ref().unwrap().phase, Phase::Interrupted);
                assert!(!s.audio.wanted);
            });
        });
    }
}
