//! One helper-owned thread retains immutable weights; turn scratch dies after each join.
//! Uses transcribe.cpp's native committed-prefix stream (Hex's warm-session design
//! informed this lifecycle; see the bundled Hex MIT notice).
use crate::{ModelId, dictation_model};
use std::{path::Path, sync::mpsc, time::Duration};
use transcribe_cpp::{
    Backend, CancelToken, CommitPolicy, Model, ModelOptions, ParakeetVocabularyOptions,
    RunExtension, RunOptions, StreamOptions, TimestampKind,
};
use vorkspace_core::dictation_words::{Corrector, Rule};
pub(crate) mod eou;
mod tdt_stream;

pub fn realtime_supported() -> bool {
    eou::supported()
}

#[cfg(debug_assertions)]
static MODEL_LOADS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(debug_assertions)]
static TDT_FIXTURE_METRICS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(debug_assertions)]
static UNIFIED_FIXTURE_TRACE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(debug_assertions)]
pub fn model_loads() -> usize {
    MODEL_LOADS.load(std::sync::atomic::Ordering::Relaxed)
}
#[cfg(debug_assertions)]
pub fn enable_tdt_fixture_metrics() {
    TDT_FIXTURE_METRICS.store(true, std::sync::atomic::Ordering::Relaxed);
}
#[cfg(debug_assertions)]
pub fn enable_unified_fixture_trace() {
    UNIFIED_FIXTURE_TRACE.store(true, std::sync::atomic::Ordering::Relaxed);
}
pub const SAMPLE_RATE: usize = 16000;
pub const MAX_SAMPLES: usize = SAMPLE_RATE * 120;
// Fixed 100 ms blocks make the queue's memory and delay bounds independent of
// how often the capture owner happens to drain its native PCM buffer.
const QUEUED_BLOCK_SAMPLES: usize = SAMPLE_RATE / 10;
const QUEUED_BLOCKS: usize = 80; // at most eight seconds of pending PCM
pub enum Input {
    Audio(Vec<f32>),
    Finish,
}
pub struct Turn {
    pub epoch: u64,
    pub cancel: CancelToken,
    pub input: mpsc::Receiver<Input>,
    pub words: Vec<vorkspace_core::dictation_words::Rule>,
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
        if pcm.len() <= QUEUED_BLOCK_SAMPLES {
            return self.send(Input::Audio(pcm));
        }
        for block in pcm.chunks(QUEUED_BLOCK_SAMPLES) {
            self.send(Input::Audio(block.to_vec()))?;
        }
        Ok(())
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
    #[cfg(debug_assertions)]
    pub fn text(&self) -> &str {
        &self.text
    }
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
    model: WarmEngine,
}
enum WarmEngine {
    Gguf(Model),
    Realtime(eou::Engine),
}
impl WarmModel {
    pub fn load(id: ModelId, path: &Path, cancel: &CancelToken) -> Result<Self, String> {
        dictation_model::verify(id, path, cancel)?;
        if id == ModelId::ParakeetRealtimeEou {
            let model = eou::Engine::load(path)?;
            #[cfg(debug_assertions)]
            MODEL_LOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if cancel.is_cancelled() {
                return Err("Cancelled".into());
            }
            model.reset()?;
            model.feed(&vec![0.; SAMPLE_RATE])?;
            if cancel.is_cancelled() {
                return Err("Cancelled".into());
            }
            model.finish()?;
            model.reset()?;
            if cancel.is_cancelled() {
                return Err("Cancelled".into());
            }
            return Ok(Self {
                id,
                model: WarmEngine::Realtime(model),
            });
        }
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
        } else {
            let mut session = model.session().map_err(|e| e.to_string())?;
            session.set_cancel_token(cancel);
            session
                .run(&vec![0.; SAMPLE_RATE], &RunOptions::default())
                .map_err(|e| format!("Could not prepare local TDT model: {e}"))?;
        }
        if cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        Ok(Self {
            id,
            model: WarmEngine::Gguf(model),
        })
    }
    pub fn turn(
        &self,
        turn: &Turn,
        mut emit: impl FnMut(Event) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut publisher = Publisher::new(turn.words.clone());
        if let WarmEngine::Realtime(model) = &self.model {
            model.reset()?;
            let mut count = 0;
            let mut tail = StableTail::default();
            emit(Event::Armed(turn.epoch))?;
            loop {
                match receive(turn, &mut count)? {
                    Input::Audio(pcm) => {
                        let text = model.feed(&pcm)?;
                        let stable = tail.observe(&text, &pcm, &publisher.source, false);
                        if tail.quiet_samples >= SAMPLE_RATE {
                            publisher.advance_stable(turn, stable, &mut emit)?;
                        } else {
                            publisher.advance(turn, stable, false, &mut emit)?;
                        }
                    }
                    Input::Finish => {
                        let text = model.finish()?;
                        let stable = tail.observe(&text, &[], &publisher.source, true);
                        publisher.advance(turn, stable, true, &mut emit)?;
                        return Ok(());
                    }
                }
            }
        }
        let WarmEngine::Gguf(model) = &self.model else {
            unreachable!()
        };
        let mut session = model.session().map_err(|e| e.to_string())?;
        session.set_cancel_token(&turn.cancel);
        let run = run_options(self.id, &turn.words);
        let mut count = 0;
        let mut raw_prefix = CommittedPrefix::default();
        if self.id == ModelId::UnifiedEnglish {
            let mut tail = StableTail::default();
            #[cfg(debug_assertions)]
            let mut previous_trace = String::new();
            let mut stream = session
                .stream(&run, &stream_options())
                .map_err(|e| format!("This model could not start native streaming: {e}"))?;
            emit(Event::Armed(turn.epoch))?;
            loop {
                match receive(turn, &mut count)? {
                    Input::Audio(pcm) => {
                        stream.feed(&pcm).map_err(|e| e.to_string())?;
                        let snapshot = stream.text();
                        let text = snapshot.committed;
                        raw_prefix.advance(&text)?;
                        let stable = tail.observe(&text, &pcm, &publisher.source, false);
                        #[cfg(debug_assertions)]
                        if UNIFIED_FIXTURE_TRACE.load(std::sync::atomic::Ordering::Relaxed) {
                            let line = format!(
                                "full={:?} committed={text:?} tentative={:?} held={stable:?} published={:?}",
                                snapshot.full, snapshot.tentative, publisher.sent.text
                            );
                            if line != previous_trace {
                                eprintln!("fixture_stream {line}");
                                previous_trace = line;
                            }
                        }
                        if tail.quiet_samples >= SAMPLE_RATE {
                            publisher.advance_stable(turn, stable, &mut emit)?;
                        } else {
                            publisher.advance(turn, stable, false, &mut emit)?;
                        }
                    }
                    Input::Finish => {
                        stream.finalize().map_err(|e| e.to_string())?;
                        let snapshot = stream.text();
                        let text = snapshot.committed;
                        #[cfg(debug_assertions)]
                        if UNIFIED_FIXTURE_TRACE.load(std::sync::atomic::Ordering::Relaxed) {
                            eprintln!(
                                "fixture_final full={:?} committed={text:?} tentative={:?} source={:?} published={:?}",
                                snapshot.full,
                                snapshot.tentative,
                                publisher.source.text,
                                publisher.sent.text
                            );
                        }
                        // The stream's committed projection can remain frozen if
                        // final decoding revises its private suffix. The source
                        // prefix already handed to the correction pipeline is a
                        // conservative boundary, even if corrections still hold
                        // some of it. Reject any final rewrite across that boundary.
                        finish_unified(
                            turn,
                            &mut tail,
                            &mut publisher,
                            &mut raw_prefix,
                            &text,
                            &snapshot.full,
                            &mut emit,
                        )?;
                        return Ok(());
                    }
                }
            }
        } else {
            let mut incremental = tdt_stream::TdtStream::new();
            let options = RunOptions {
                timestamps: TimestampKind::Word,
                ..run
            };
            let mut decode =
                |audio: &[f32]| session.run(audio, &options).map_err(|e| e.to_string());
            emit(Event::Armed(turn.epoch))?;
            loop {
                let (text, finished) = match receive(turn, &mut count)? {
                    Input::Audio(block) => (incremental.feed(&block, &mut decode)?, false),
                    Input::Finish => (incremental.finish(&mut decode)?, true),
                };
                if let Some(text) = text {
                    publisher.advance_segment(turn, &text, finished, &mut emit)?;
                } else if finished {
                    let text = publisher.source.text.clone();
                    publisher.advance(turn, &text, true, &mut emit)?;
                }
                if finished {
                    return Ok(());
                }
            }
        }
    }
}

fn finish_unified(
    turn: &Turn,
    tail: &mut StableTail,
    publisher: &mut Publisher,
    raw_prefix: &mut CommittedPrefix,
    committed: &str,
    final_text: &str,
    emit: &mut impl FnMut(Event) -> Result<(), String>,
) -> Result<(), String> {
    raw_prefix.advance(committed)?;
    let stable = tail.observe(final_text, &[], &publisher.source, true);
    publisher.advance(turn, stable, true, emit)
}

fn run_options(id: ModelId, rules: &[Rule]) -> RunOptions {
    let mut vocabulary = Vec::<String>::new();
    let mut sounds_like = Vec::new();
    // Unified's pinned decoder rescores the entire stream at finalization,
    // which can revise published text. TDT scores each window before it is
    // committed. All engines still receive deterministic local corrections.
    for rule in rules.iter().filter(|_| id == ModelId::ParakeetTdtV3) {
        let index = vocabulary
            .iter()
            .position(|word| word == &rule.preferred)
            .unwrap_or_else(|| {
                vocabulary.push(rule.preferred.clone());
                vocabulary.len() - 1
            });
        sounds_like.push((index, rule.heard.clone()));
    }
    RunOptions {
        language: id.descriptor().language.map(str::to_owned),
        family: (!vocabulary.is_empty()).then_some(RunExtension::ParakeetVocabulary(
            ParakeetVocabularyOptions {
                boost: None,
                sounds_like,
            },
        )),
        vocabulary,
        ..Default::default()
    }
}

struct Publisher {
    source: CommittedPrefix,
    corrected: String,
    sent: CommittedPrefix,
    corrector: Corrector,
}
impl Publisher {
    fn new(rules: Vec<Rule>) -> Self {
        Self {
            source: CommittedPrefix::default(),
            corrected: String::new(),
            sent: CommittedPrefix::default(),
            corrector: Corrector::new(rules),
        }
    }
    fn advance(
        &mut self,
        turn: &Turn,
        text: &str,
        finished: bool,
        emit: &mut impl FnMut(Event) -> Result<(), String>,
    ) -> Result<(), String> {
        self.advance_inner(turn, text, finished, false, false, emit)
    }
    fn advance_segment(
        &mut self,
        turn: &Turn,
        text: &str,
        finished: bool,
        emit: &mut impl FnMut(Event) -> Result<(), String>,
    ) -> Result<(), String> {
        self.advance_inner(turn, text, finished, true, false, emit)
    }
    fn advance_stable(
        &mut self,
        turn: &Turn,
        text: &str,
        emit: &mut impl FnMut(Event) -> Result<(), String>,
    ) -> Result<(), String> {
        self.advance_inner(turn, text, false, false, true, emit)
    }
    fn advance_inner(
        &mut self,
        turn: &Turn,
        text: &str,
        finished: bool,
        closed: bool,
        stable: bool,
        emit: &mut impl FnMut(Event) -> Result<(), String>,
    ) -> Result<(), String> {
        if turn.cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        let suffix = self.source.advance(text)?;
        let corrected = if closed {
            self.corrector.push_segment(&suffix, finished)
        } else if stable {
            self.corrector.push_stable(&suffix)
        } else {
            self.corrector.push(&suffix, finished)
        };
        self.corrected.push_str(&corrected);
        // A future punctuation token belongs before trailing whitespace. Keep
        // that whitespace private until more text or Finish fixes its position.
        let visible = if finished {
            self.corrected.as_str()
        } else {
            self.corrected.trim_end_matches(char::is_whitespace)
        };
        publish(turn, &mut self.sent, visible, emit)
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

/// A streaming model can finish a single word without emitting a trailing
/// space. Release its immutable prefix after one second of quiet, while never
/// shortening text that has already left the worker.
#[derive(Default)]
struct StableTail {
    previous: String,
    quiet_samples: usize,
    levels: QuietTracker,
}
impl StableTail {
    fn observe<'a>(
        &mut self,
        text: &'a str,
        pcm: &[f32],
        delivered: &CommittedPrefix,
        finished: bool,
    ) -> &'a str {
        let quiet = self.levels.observe(pcm);
        if text != self.previous {
            self.previous = text.to_owned();
            self.quiet_samples = 0;
        } else if quiet {
            self.quiet_samples = self.quiet_samples.saturating_add(pcm.len());
        } else {
            self.quiet_samples = 0;
        }
        let candidate = stable_words(text, finished || self.quiet_samples >= SAMPLE_RATE);
        let candidate = if finished {
            candidate
        } else {
            candidate.trim_end_matches(char::is_whitespace)
        };
        if candidate.len() < delivered.text.len() && text.starts_with(&delivered.text) {
            &text[..delivered.text.len()]
        } else {
            candidate
        }
    }
}

/// A fixed absolute silence gate can hold the last word forever in a room with
/// a steady noise floor. Compare against recent speech too, with a strict cap;
/// a word still needs the existing text-stability and duration checks.
#[derive(Default)]
struct QuietTracker {
    levels: std::collections::VecDeque<(usize, f64)>,
    samples: usize,
}
impl QuietTracker {
    fn observe(&mut self, pcm: &[f32]) -> bool {
        if pcm.is_empty() || pcm.iter().any(|sample| !sample.is_finite()) {
            return false;
        }
        let rms = (pcm
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / pcm.len() as f64)
            .sqrt();
        let reference = self
            .levels
            .iter()
            .map(|(_, level)| *level)
            .fold(0.0_f64, f64::max);
        let threshold = (reference * 0.2).clamp(0.005, 0.02);
        self.levels.push_back((pcm.len(), rms));
        self.samples = self.samples.saturating_add(pcm.len());
        while self.samples > SAMPLE_RATE * 3 && self.levels.len() > 1 {
            self.samples -= self.levels.pop_front().unwrap().0;
        }
        rms <= threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_tdt_gets_acoustic_vocabulary_and_aliases() {
        let rules = vorkspace_core::dictation_words::parse(
            "Kennedy\nwork space => Vorkspace\nvork space => Vorkspace",
        )
        .unwrap();
        {
            let run = run_options(ModelId::ParakeetTdtV3, &rules);
            assert_eq!(run.vocabulary, ["Kennedy", "Vorkspace"]);
            let Some(RunExtension::ParakeetVocabulary(options)) = run.family else {
                panic!("missing acoustic vocabulary")
            };
            assert_eq!(
                options.sounds_like,
                [
                    (0, "Kennedy".into()),
                    (1, "work space".into()),
                    (1, "vork space".into())
                ]
            );
        }
        let unified = run_options(ModelId::UnifiedEnglish, &rules);
        assert!(unified.vocabulary.is_empty());
        assert!(unified.family.is_none());
    }
    #[test]
    fn corrected_cumulative_events_are_append_only_and_cancellation_drops_the_tail() {
        let (_pipe, input) = Pipe::new();
        let turn = Turn {
            epoch: 9,
            cancel: CancelToken::new(),
            input,
            words: Vec::new(),
        };
        let mut publisher = Publisher::new(
            vorkspace_core::dictation_words::parse("work space => Vorkspace").unwrap(),
        );
        let mut events = Vec::new();
        publisher
            .advance(&turn, "Say work ", false, &mut |e| {
                events.push(e);
                Ok(())
            })
            .unwrap();
        publisher
            .advance(&turn, "Say work space, now", false, &mut |e| {
                events.push(e);
                Ok(())
            })
            .unwrap();
        let mut received = CommittedPrefix::default();
        let mut result = String::new();
        for event in events {
            if let Event::Committed(epoch, text) = event {
                assert_eq!(epoch, 9);
                result.push_str(&received.advance(&text).unwrap());
            }
        }
        assert_eq!(result, "Say Vorkspace,");
        turn.cancel.cancel();
        assert!(
            publisher
                .advance(&turn, "Say work space, now!", true, &mut |_| panic!(
                    "cancelled publication"
                ))
                .is_err()
        );
    }
    #[test]
    fn continuous_chunks_correct_a_cross_chunk_phrase_without_final_replay() {
        let (_pipe, input) = Pipe::new();
        let turn = Turn {
            epoch: 10,
            cancel: CancelToken::new(),
            input,
            words: Vec::new(),
        };
        let mut publisher = Publisher::new(
            vorkspace_core::dictation_words::parse("Kennedy\nfellow Americans => Fellow-Americans")
                .unwrap(),
        );
        let mut values = Vec::new();
        for (text, final_text) in [
            ("And so, my fellow ", false),
            ("And so, my fellow Amer", false),
            ("And so, my fellow Americans, ask ", false),
            ("And so, my fellow Americans, ask not.", true),
        ] {
            publisher
                .advance(&turn, text, final_text, &mut |event| {
                    if let Event::Committed(_, text) = event {
                        values.push(text);
                    }
                    Ok(())
                })
                .unwrap();
        }
        assert!(values.len() >= 2);
        assert!(values[0].contains("And so"));
        assert!(values.windows(2).all(|pair| pair[1].starts_with(&pair[0])));
        assert_eq!(
            values.last().unwrap(),
            "And so, my Fellow-Americans, ask not."
        );
    }
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
    fn final_model_hypothesis_extends_only_the_actually_published_prefix() {
        let (_pipe, input) = Pipe::new();
        let turn = Turn {
            epoch: 11,
            cancel: CancelToken::new(),
            input,
            words: Vec::new(),
        };
        let mut publisher = Publisher::new(Vec::new());
        let mut tail = StableTail::default();
        let mut raw_prefix = CommittedPrefix::default();
        raw_prefix.advance("Héllo ").unwrap();
        let mut values = Vec::new();
        let quiet = vec![0.; SAMPLE_RATE / 10];
        let source = CommittedPrefix::default();
        let stable = tail.observe("Héllo ", &quiet, &source, false);
        assert_eq!(stable, "Héllo");
        publisher
            .advance(&turn, stable, false, &mut |event| {
                if let Event::Committed(_, text) = event {
                    values.push(text);
                }
                Ok(())
            })
            .unwrap();
        // Simulates the native stream retaining its old committed projection
        // while its final full hypothesis supplies punctuation and spaces.
        finish_unified(
            &turn,
            &mut tail,
            &mut publisher,
            &mut raw_prefix,
            "Héllo ",
            "Héllo?  ",
            &mut |event| {
                if let Event::Committed(_, text) = event {
                    values.push(text);
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(values, ["Héllo", "Héllo?  "]);
        let mut received = CommittedPrefix::default();
        assert_eq!(received.advance(&values[0]).unwrap(), "Héllo");
        assert_eq!(received.advance(&values[1]).unwrap(), "?  ");
        assert!(
            finish_unified(
                &turn,
                &mut tail,
                &mut publisher,
                &mut raw_prefix,
                "Héllo ",
                "Changed?",
                &mut |_| panic!("rewrite")
            )
            .is_err()
        );
    }

    #[test]
    fn final_punctuation_closes_a_custom_alias_without_replay() {
        let (_pipe, input) = Pipe::new();
        let turn = Turn {
            epoch: 12,
            cancel: CancelToken::new(),
            input,
            words: Vec::new(),
        };
        let mut publisher = Publisher::new(
            vorkspace_core::dictation_words::parse("work space => Vorkspace").unwrap(),
        );
        let mut tail = StableTail::default();
        let mut raw_prefix = CommittedPrefix::default();
        let mut values = Vec::new();
        for (text, finished) in [("Open work", false), ("Open work space? ", true)] {
            let mut emit = |event| {
                if let Event::Committed(_, text) = event {
                    values.push(text);
                }
                Ok(())
            };
            if finished {
                finish_unified(
                    &turn,
                    &mut tail,
                    &mut publisher,
                    &mut raw_prefix,
                    text,
                    text,
                    &mut emit,
                )
                .unwrap();
            } else {
                raw_prefix.advance(text).unwrap();
                publisher.advance(&turn, text, false, &mut emit).unwrap();
            }
        }
        assert_eq!(values, ["Open", "Open Vorkspace? "]);
    }
    #[test]
    fn cancellation_before_and_after_partial_publication_never_replays() {
        let (_pipe, input) = Pipe::new();
        let turn = Turn {
            epoch: 7,
            cancel: CancelToken::new(),
            input,
            words: Vec::new(),
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
    fn a_single_stable_word_is_released_after_quiet_without_retracting_a_prefix() {
        let mut tail = StableTail::default();
        let mut delivered = CommittedPrefix::default();
        let quiet = vec![0.; SAMPLE_RATE / 10];
        let speech = vec![0.1; SAMPLE_RATE / 10];
        for _ in 0..15 {
            assert_eq!(tail.observe("hello", &speech, &delivered, false), "");
        }
        assert_eq!(tail.observe("hello", &quiet, &delivered, false), "");
        for _ in 0..8 {
            assert_eq!(tail.observe("hello", &quiet, &delivered, false), "");
        }
        let stable = tail.observe("hello", &quiet, &delivered, false);
        assert_eq!(stable, "hello");
        delivered.advance(stable).unwrap();
        assert_eq!(tail.observe("helloing", &quiet, &delivered, false), "hello");
        assert_eq!(
            tail.observe("helloing world", &quiet, &delivered, false),
            "helloing"
        );
        assert_eq!(
            tail.observe("helloing world", &[], &delivered, true),
            "helloing world"
        );
    }
    #[test]
    fn steady_background_noise_does_not_hold_the_last_stable_word() {
        let mut tail = StableTail::default();
        let delivered = CommittedPrefix::default();
        let speech = vec![0.1; SAMPLE_RATE / 10];
        let noise = vec![0.01; SAMPLE_RATE / 10];
        for _ in 0..15 {
            assert_eq!(tail.observe("hello", &speech, &delivered, false), "");
        }
        for _ in 0..9 {
            assert_eq!(tail.observe("hello", &noise, &delivered, false), "");
        }
        assert_eq!(tail.observe("hello", &noise, &delivered, false), "hello");
        assert_eq!(
            tail.observe("hello there", &speech, &delivered, false),
            "hello"
        );
        assert_eq!(
            tail.observe("hello there  ", &[], &delivered, true),
            "hello there  "
        );
    }
    #[test]
    fn queue_pressure_is_explicit_and_finish_is_ordered() {
        let (pipe, rx) = Pipe::new();
        for _ in 0..QUEUED_BLOCKS {
            pipe.audio(vec![0.; QUEUED_BLOCK_SAMPLES]).unwrap();
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
    fn large_capture_drains_cannot_exceed_the_same_eight_second_queue() {
        let (pipe, rx) = Pipe::new();
        for _ in 0..8 {
            pipe.audio(vec![0.5; SAMPLE_RATE]).unwrap();
        }
        assert!(pipe.audio(vec![0.5]).is_err());
        let mut received = Vec::new();
        while let Ok(Input::Audio(block)) = rx.try_recv() {
            received.extend(block);
        }
        assert_eq!(received, vec![0.5; SAMPLE_RATE * 8]);
    }
    #[test]
    fn cancellation_and_audio_bounds_fail_closed() {
        let (pipe, rx) = Pipe::new();
        let turn = Turn {
            epoch: 1,
            cancel: CancelToken::new(),
            input: rx,
            words: Vec::new(),
        };
        pipe.audio(vec![0.; SAMPLE_RATE]).unwrap();
        assert!(receive(&turn, &mut MAX_SAMPLES.clone()).is_err());
        turn.cancel.cancel();
        assert!(receive(&turn, &mut 0).is_err());
        assert!(pipe.audio(vec![0.; SAMPLE_RATE + 1]).is_err());
    }
}
