//! One helper-owned thread retains immutable weights; turn scratch dies after each join.
//! Uses transcribe.cpp's native committed-prefix stream (Hex's warm-session design
//! informed this lifecycle; see the bundled Hex MIT notice).
use crate::{ModelId, dictation_model};
use std::{path::Path, sync::mpsc, time::Duration};
use transcribe_cpp::{
    Backend, CancelToken, CommitPolicy, Model, ModelOptions, RunOptions, StreamOptions,
};

#[cfg(debug_assertions)]
static MODEL_LOADS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(debug_assertions)]
pub fn model_loads() -> usize {
    MODEL_LOADS.load(std::sync::atomic::Ordering::Relaxed)
}
pub const SAMPLE_RATE: usize = 16000;
pub const MAX_SAMPLES: usize = SAMPLE_RATE * 120;
const QUEUED_BLOCKS: usize = 16; // at most 16 seconds, each block <= one second
pub enum Input {
    Audio(Vec<f32>),
    Finish,
}
pub struct Turn {
    pub epoch: u64,
    pub cancel: CancelToken,
    pub input: mpsc::Receiver<Input>,
}
pub enum Event {
    Ready,
    Armed(u64),
    Committed(u64, String),
    Done(u64, Result<(), String>),
}
pub struct Pipe {
    tx: mpsc::SyncSender<Input>,
}
impl Pipe {
    pub fn new() -> (Self, mpsc::Receiver<Input>) {
        let (tx, rx) = mpsc::sync_channel(QUEUED_BLOCKS);
        (Self { tx }, rx)
    }
    pub fn audio(&self, pcm: Vec<f32>) -> Result<(), String> {
        if pcm.len() > SAMPLE_RATE {
            return Err("Audio block exceeded its bound".into());
        }
        if pcm.is_empty() {
            return Ok(());
        }
        self.send(Input::Audio(pcm))
    }
    pub fn finish(&self) -> Result<(), String> {
        self.send(Input::Finish)
    }
    fn send(&self, input: Input) -> Result<(), String> {
        self.tx.try_send(input).map_err(|_| "Local dictation could not keep up. Further insertion stopped; text already inserted is kept. Try a shorter recording.".into())
    }
}
/// Advance before attempting an external effect. Neither uncertainty nor an error
/// grants permission to replay. Byte offsets always come from valid UTF-8 prefixes.
#[derive(Default, Clone)]
pub struct CommittedPrefix {
    text: String,
}
impl CommittedPrefix {
    pub fn advance(&mut self, text: &str) -> Result<String, String> {
        let suffix = text
            .strip_prefix(&self.text)
            .ok_or(
                "Local dictation changed an already committed prefix. Further insertion stopped.",
            )?
            .to_owned();
        self.text = text.to_owned();
        Ok(suffix)
    }
}

pub struct WarmModel {
    id: ModelId,
    model: Model,
}
impl WarmModel {
    pub fn load(id: ModelId, path: &Path, cancel: &CancelToken) -> Result<Self, String> {
        dictation_model::verify(id, path, cancel)?;
        transcribe_cpp::disable_logging();
        let model = Model::load_with(
            path,
            &ModelOptions {
                backend: Backend::Metal,
                ..Default::default()
            },
        )
        .map_err(|e| format!("Could not prepare local model: {e}"))?;
        #[cfg(debug_assertions)]
        MODEL_LOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        // Probe and prewarm using synthetic silence only. A cold Metal graph
        // belongs to Preparing, never the user's first accepted microphone turn.
        if id == ModelId::UnifiedEnglish {
            let capabilities = model.capabilities().map_err(|e| e.to_string())?;
            if !capabilities.supports_streaming {
                return Err("Selected model does not support native streaming".into());
            }
            let mut session = model.session().map_err(|e| e.to_string())?;
            session.set_cancel_token(cancel);
            let mut stream = session
                .stream(
                    &RunOptions {
                        language: Some("en".into()),
                        ..Default::default()
                    },
                    &stream_options(),
                )
                .map_err(|e| e.to_string())?;
            stream
                .feed(&vec![0.; SAMPLE_RATE * 3])
                .map_err(|e| e.to_string())?;
            stream.finalize().map_err(|e| e.to_string())?;
        }
        if cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        Ok(Self { id, model })
    }
    pub fn turn(
        &self,
        turn: &Turn,
        mut emit: impl FnMut(Event) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut session = self.model.session().map_err(|e| e.to_string())?;
        session.set_cancel_token(&turn.cancel);
        let run = RunOptions {
            language: self.id.descriptor().language.map(str::to_owned),
            ..Default::default()
        };
        let mut count = 0;
        let mut prefix = CommittedPrefix::default();
        let mut raw_prefix = CommittedPrefix::default();
        if self.id == ModelId::UnifiedEnglish {
            let mut stream = session
                .stream(&run, &stream_options())
                .map_err(|e| format!("This model could not start native streaming: {e}"))?;
            emit(Event::Armed(turn.epoch))?;
            loop {
                match receive(turn, &mut count)? {
                    Input::Audio(pcm) => {
                        stream.feed(&pcm).map_err(|e| e.to_string())?;
                    }
                    Input::Finish => {
                        stream.finalize().map_err(|e| e.to_string())?;
                        raw_prefix.advance(&stream.text().committed)?;
                        publish(
                            turn,
                            &mut prefix,
                            stable_words(&stream.text().committed, true),
                            &mut emit,
                        )?;
                        return Ok(());
                    }
                }
                raw_prefix.advance(&stream.text().committed)?;
                publish(
                    turn,
                    &mut prefix,
                    stable_words(&stream.text().committed, false),
                    &mut emit,
                )?;
            }
        } else {
            // The pinned TDT artifact is offline-only. Never simulate streaming by
            // rerunning a growing clip; retain bounded PCM and infer once at stop.
            emit(Event::Armed(turn.epoch))?;
            let mut pcm = Vec::new();
            while let Input::Audio(block) = receive(turn, &mut count)? {
                pcm.extend(block);
            }
            let mut text = String::new();
            for chunk in pcm.chunks(SAMPLE_RATE * 20) {
                if turn.cancel.is_cancelled() {
                    return Err("Cancelled".into());
                }
                if chunk.len() < 1600 || chunk.iter().all(|v| v.abs() < 0.001) {
                    continue;
                }
                let mut padded = chunk.to_vec();
                padded.resize(padded.len().max(SAMPLE_RATE), 0.);
                let result = session.run(&padded, &run).map_err(|e| e.to_string())?;
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(result.text.trim());
            }
            publish(turn, &mut prefix, &text, &mut emit)
        }
    }
}
fn publish(
    turn: &Turn,
    prefix: &mut CommittedPrefix,
    text: &str,
    emit: &mut impl FnMut(Event) -> Result<(), String>,
) -> Result<(), String> {
    if turn.cancel.is_cancelled() {
        return Err("Cancelled".into());
    }
    if !prefix.advance(text)?.is_empty() {
        emit(Event::Committed(turn.epoch, text.to_owned()))?;
    }
    Ok(())
}
fn receive(turn: &Turn, count: &mut usize) -> Result<Input, String> {
    loop {
        if turn.cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        match turn.input.recv_timeout(Duration::from_millis(50)) {
            Ok(Input::Audio(pcm)) => {
                *count += pcm.len();
                if *count > MAX_SAMPLES || pcm.len() > SAMPLE_RATE {
                    return Err("Recording exceeded its audio bound".into());
                }
                return Ok(Input::Audio(pcm));
            }
            Ok(Input::Finish) => return Ok(Input::Finish),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("Cancelled".into()),
        }
    }
}
/// Returns only after the thread has freed weights. The caller joins off GPUI
/// and fences all publications by configuration generation and attempt epoch.
pub fn run(
    id: ModelId,
    path: &Path,
    cancel: &CancelToken,
    turns: mpsc::Receiver<Turn>,
    events: async_channel::Sender<Event>,
) -> Result<(), String> {
    let model = WarmModel::load(id, path, cancel)?;
    events
        .send_blocking(Event::Ready)
        .map_err(|e| e.to_string())?;
    while !cancel.is_cancelled() {
        let turn = match turns.recv_timeout(Duration::from_millis(50)) {
            Ok(turn) => turn,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        let result = model.turn(&turn, |event| {
            events
                .try_send(event)
                .map_err(|_| "Dictation delivery queue is full; insertion stopped".into())
        });
        // Completion is essential for releasing admission. The main loop always
        // drains this bounded channel, including after cancellation/retirement.
        events
            .send_blocking(Event::Done(turn.epoch, result))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn stream_options() -> StreamOptions {
    StreamOptions {
        commit_policy: CommitPolicy::StablePrefix,
        stable_prefix_agreement_n: 3,
        // Pinned Unified menu: 80 ms frames, C=7 and R=7. Keep 5.6 s
        // left context; advance 560 ms per native bounded encoder window.
        family: Some(transcribe_cpp::StreamExtension::ParakeetBuffered(
            transcribe_cpp::ParakeetBufferedStreamOptions {
                left_ms: None,
                chunk_ms: Some(560),
                right_ms: Some(560),
            },
        )),
    }
}

fn stable_words(text: &str, final_text: bool) -> &str {
    if final_text {
        return text;
    }
    let end = text
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_prefix_and_final_suffix_are_once_only() {
        let mut p = CommittedPrefix::default();
        assert_eq!(p.advance(" Héllo").unwrap(), " Héllo");
        assert_eq!(p.advance(" Héllo 世界").unwrap(), " 世界");
        assert_eq!(p.advance(" Héllo 世界.").unwrap(), ".");
        assert_eq!(p.advance(" Héllo 世界.").unwrap(), "");
        assert!(p.advance(" Hello").is_err());
    }
    #[test]
    fn cancellation_before_and_after_partial_publication_never_replays() {
        let (_pipe, input) = Pipe::new();
        let turn = Turn {
            epoch: 7,
            cancel: CancelToken::new(),
            input,
        };
        let mut prefix = CommittedPrefix::default();
        let mut effects = Vec::new();
        publish(&turn, &mut prefix, "stable ", &mut |e| {
            effects.push(e);
            Ok(())
        })
        .unwrap();
        publish(&turn, &mut prefix, "stable ", &mut |e| {
            effects.push(e);
            Ok(())
        })
        .unwrap();
        assert_eq!(effects.len(), 1);
        assert!(
            publish(&turn, &mut prefix, "revised ", &mut |e| {
                effects.push(e);
                Ok(())
            })
            .is_err()
        );
        turn.cancel.cancel();
        assert!(
            publish(&turn, &mut prefix, "stable suffix", &mut |e| {
                effects.push(e);
                Ok(())
            })
            .is_err()
        );
        assert_eq!(effects.len(), 1);
        let mut fresh = CommittedPrefix::default();
        assert!(
            publish(&turn, &mut fresh, "anything", &mut |_| panic!(
                "cancelled before first publication"
            ))
            .is_err()
        );
    }
    #[test]
    fn incomplete_words_are_held_until_boundary_or_finalization() {
        assert_eq!(
            stable_words("And my fellow Americ", false),
            "And my fellow "
        );
        assert_eq!(
            stable_words("And my fellow Americans, ", false),
            "And my fellow Americans, "
        );
        assert_eq!(stable_words("café", false), "");
        assert_eq!(stable_words("café", true), "café");
        assert_eq!(stable_words("café\u{2003}世", false), "café\u{2003}");
    }
    #[test]
    fn queue_pressure_is_explicit_and_finish_is_ordered() {
        let (pipe, rx) = Pipe::new();
        for _ in 0..QUEUED_BLOCKS {
            pipe.audio(vec![0.; SAMPLE_RATE]).unwrap();
        }
        assert!(pipe.audio(vec![0.]).is_err());
        assert!(pipe.finish().is_err());
        assert!(matches!(rx.recv().unwrap(), Input::Audio(_)));
        pipe.finish().unwrap();
        for _ in 1..QUEUED_BLOCKS {
            assert!(matches!(rx.recv().unwrap(), Input::Audio(_)));
        }
        assert!(matches!(rx.recv().unwrap(), Input::Finish));
    }
    #[test]
    fn cancellation_and_audio_bounds_fail_closed() {
        let (pipe, rx) = Pipe::new();
        let turn = Turn {
            epoch: 1,
            cancel: CancelToken::new(),
            input: rx,
        };
        pipe.audio(vec![0.; SAMPLE_RATE]).unwrap();
        assert!(receive(&turn, &mut MAX_SAMPLES.clone()).is_err());
        turn.cancel.cancel();
        assert!(receive(&turn, &mut 0).is_err());
        assert!(pipe.audio(vec![0.; SAMPLE_RATE + 1]).is_err());
    }
}
