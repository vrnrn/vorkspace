//! Bounded, overlap-aware incremental inference for the offline TDT model.
//!
//! Each preview reuses the current audio window and the warm model. Only words
//! repeated across several previews can be published. Once text has left the
//! helper, a later hypothesis must match its timed tail before adding anything.
use std::collections::VecDeque;
use transcribe_cpp::{Transcript, Word};

use super::{MAX_SAMPLES, SAMPLE_RATE};

const PREVIEW_INTERVAL: usize = SAMPLE_RATE * 3 / 5;
const MIN_PREVIEW: usize = SAMPLE_RATE;
const WINDOW_TARGET: usize = SAMPLE_RATE * 12;
const WINDOW_LIMIT: usize = SAMPLE_RATE * 24;
const CONTEXT: usize = SAMPLE_RATE * 2;
const QUIET_CONTEXT: usize = SAMPLE_RATE * 4;
const FRAME: usize = SAMPLE_RATE * 80 / 1000;
const AGREEMENTS: usize = 3;
const RIGHT_CONTEXT_MS: i64 = 640;
const TRAILING_WORD_HOLD_MS: i64 = 2000;
const ALIGN_TOLERANCE_MS: i64 = 1200;

#[derive(Clone, Debug)]
struct TimedWord {
    text: String,
    start: i64,
    end: i64,
}

impl TimedWord {
    fn from_result(word: &Word, origin_ms: i64, duration_ms: i64) -> Result<Self, String> {
        if word.t0_ms < 0 || word.t1_ms < word.t0_ms || word.t0_ms > duration_ms + 160 {
            return Err("Local dictation returned invalid word timing".into());
        }
        let text = word.text.trim().to_owned();
        if text.is_empty() {
            return Err("Local dictation returned an empty timed word".into());
        }
        Ok(Self {
            text,
            start: origin_ms + word.t0_ms.min(duration_ms),
            end: origin_ms + word.t1_ms.min(duration_ms),
        })
    }

    fn anchor_key(&self) -> String {
        self.text
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase()
    }
}

pub(super) struct TdtStream {
    audio: Vec<f32>,
    window_start: usize,
    last_preview: usize,
    quiet_samples: usize,
    levels: super::QuietTracker,
    recent: VecDeque<Vec<TimedWord>>,
    published: Vec<TimedWord>,
    text: String,
    #[cfg(debug_assertions)]
    mismatch_logged: bool,
}

impl TdtStream {
    pub(super) fn new() -> Self {
        Self {
            audio: Vec::new(),
            window_start: 0,
            last_preview: 0,
            quiet_samples: 0,
            levels: super::QuietTracker::default(),
            recent: VecDeque::new(),
            published: Vec::new(),
            text: String::new(),
            #[cfg(debug_assertions)]
            mismatch_logged: false,
        }
    }

    pub(super) fn feed(
        &mut self,
        pcm: &[f32],
        decode: &mut impl FnMut(&[f32]) -> Result<Transcript, String>,
    ) -> Result<Option<String>, String> {
        if self.audio.len().saturating_add(pcm.len()) > MAX_SAMPLES {
            return Err("Recording exceeded its audio bound".into());
        }
        self.audio.extend_from_slice(pcm);
        if self.levels.observe(pcm) {
            self.quiet_samples = self.quiet_samples.saturating_add(pcm.len());
        } else {
            self.quiet_samples = 0;
        }
        if self.audio.len() < MIN_PREVIEW
            || self.audio.len().saturating_sub(self.last_preview) < PREVIEW_INTERVAL
        {
            return Ok(None);
        }
        self.preview(false, decode)
    }

    pub(super) fn finish(
        &mut self,
        decode: &mut impl FnMut(&[f32]) -> Result<Transcript, String>,
    ) -> Result<Option<String>, String> {
        if self.audio.len() < 1600 || self.audio.iter().all(|sample| sample.abs() < 0.001) {
            return Ok(None);
        }
        self.preview(true, decode)
    }

    fn preview(
        &mut self,
        final_audio: bool,
        decode: &mut impl FnMut(&[f32]) -> Result<Transcript, String>,
    ) -> Result<Option<String>, String> {
        self.last_preview = self.audio.len();
        let start = self.window_start;
        let input = &self.audio[start..];
        // The offline model expects at least one second of samples. Padding is
        // temporary and never enters the retained recording or timing boundary.
        let mut padded = Vec::new();
        let input = if input.len() < SAMPLE_RATE {
            padded.extend_from_slice(input);
            padded.resize(SAMPLE_RATE, 0.0);
            padded.as_slice()
        } else {
            input
        };
        #[cfg(debug_assertions)]
        let began = std::time::Instant::now();
        let result = decode(input)?;
        #[cfg(debug_assertions)]
        if super::TDT_FIXTURE_METRICS.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!(
                "tdt_window_ms={} inference_ms={} final={final_audio}",
                samples_ms(input.len()),
                began.elapsed().as_millis()
            );
        }
        if !result.text.trim().is_empty() && result.words.is_empty() {
            return Err("Local dictation did not provide word timing; insertion stopped".into());
        }
        let origin_ms = samples_ms(start);
        let duration_ms = samples_ms(self.audio.len() - start);
        let words = result
            .words
            .iter()
            .map(|word| TimedWord::from_result(word, origin_ms, duration_ms))
            .collect::<Result<Vec<_>, _>>()?;
        if words.windows(2).any(|w| w[0].start > w[1].start) {
            return Err("Local dictation returned unordered word timing".into());
        }
        let Some(anchor_end) = self.anchor_end(&words, origin_ms) else {
            #[cfg(debug_assertions)]
            if !self.mismatch_logged
                && super::TDT_FIXTURE_METRICS.load(std::sync::atomic::Ordering::Relaxed)
            {
                eprintln!(
                    "tdt_anchor_mismatch origin_ms={origin_ms} published_tail={:?} window_words={words:?}",
                    self.published.iter().rev().take(6).collect::<Vec<_>>()
                );
                self.mismatch_logged = true;
            }
            self.recent.clear();
            if final_audio || self.audio.len() - self.window_start >= WINDOW_LIMIT {
                return Err(
                    "Local dictation changed previously inserted words; further insertion stopped"
                        .into(),
                );
            }
            return Ok(None);
        };
        let pending = words[anchor_end..].to_vec();
        self.recent.push_back(pending);
        while self.recent.len() > AGREEMENTS {
            self.recent.pop_front();
        }
        let count = if final_audio {
            self.recent.back().map_or(0, Vec::len)
        } else {
            self.stable_count(samples_ms(self.audio.len()))
        };
        let mut changed = false;
        // If a word was emitted without unstable trailing punctuation during a
        // pause, append punctuation once the next word or final result arrives.
        if (final_audio || count > 0)
            && anchor_end > 0
            && let Some(last) = self.published.last_mut()
            && let Some(observed) = words.get(anchor_end - 1)
            && let Some(suffix) = observed.text.strip_prefix(&last.text)
            && !suffix.is_empty()
            && suffix
                .chars()
                .all(|c| !c.is_alphanumeric() && !c.is_whitespace())
        {
            self.text.push_str(suffix);
            last.text.push_str(suffix);
            changed = true;
        }
        if count > 0 {
            let current = self.recent.back().unwrap();
            let mut selected = current[..count].to_vec();
            if !final_audio && count == current.len() && self.quiet_samples >= SAMPLE_RATE {
                let last = selected.last_mut().unwrap();
                let word = last.text.trim_end_matches(|c: char| !c.is_alphanumeric());
                if !word.is_empty() {
                    last.text = word.to_owned();
                }
            }
            for word in selected {
                if !self.text.is_empty() {
                    self.text.push(' ');
                }
                self.text.push_str(&word.text);
                self.published.push(word);
            }
            for preview in &mut self.recent {
                if final_audio {
                    preview.clear();
                } else {
                    preview.drain(..count);
                }
            }
            changed = true;
        }
        if !final_audio {
            self.advance_window()?;
        }
        Ok(changed.then(|| self.text.clone()))
    }

    // Match the latest one to three already-inserted words by text and time.
    // This avoids replaying the overlap after moving the model window. If the
    // overlap is ambiguous or revised, wait rather than guessing an offset.
    fn anchor_end(&self, words: &[TimedWord], origin_ms: i64) -> Option<usize> {
        let Some(last) = self.published.last() else {
            return Some(0);
        };
        if last.end + ALIGN_TOLERANCE_MS < origin_ms {
            return Some(0);
        }
        let anchor = self.anchor_words(origin_ms);
        if anchor.is_empty() || anchor.len() > words.len() {
            return None;
        }
        words
            .windows(anchor.len())
            .enumerate()
            .filter_map(|(index, candidate)| {
                let mut distance = 0;
                for (observed, committed) in candidate.iter().zip(&anchor) {
                    let shift = (observed.start - committed.start).abs();
                    if observed.anchor_key() != committed.anchor_key() || shift > ALIGN_TOLERANCE_MS
                    {
                        return None;
                    }
                    distance += shift;
                }
                Some((distance, index + anchor.len()))
            })
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, end)| end)
    }

    fn anchor_words(&self, origin_ms: i64) -> Vec<&TimedWord> {
        let mut anchor: Vec<_> = self
            .published
            .iter()
            .rev()
            .take_while(|word| word.start >= origin_ms && !word.text.chars().any(char::is_numeric))
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        // Number formatting can change when a window moves (for example,
        // digits versus spelled-out words). Keep its original context until
        // a later nonnumeric anchor is committed or the quiet tail is safe.
        if anchor.is_empty()
            && let Some(last) = self.published.last()
            && last.start >= origin_ms
        {
            anchor.push(last);
        }
        anchor
    }

    fn stable_count(&self, audio_end_ms: i64) -> usize {
        if self.recent.len() != AGREEMENTS {
            return 0;
        }
        let current = self.recent.back().unwrap();
        let mut count = 0;
        for (index, word) in current.iter().enumerate() {
            let quiet_last = index + 1 == current.len()
                && self.quiet_samples >= SAMPLE_RATE
                && !word.text.chars().any(char::is_numeric);
            if quiet_last && word.start + RIGHT_CONTEXT_MS > audio_end_ms {
                break;
            }
            // The last word can gain punctuation or complete only after more
            // speech. Hold it longer, but do publish it after a sustained pause.
            let required_context = if index + 1 == current.len() {
                TRAILING_WORD_HOLD_MS
            } else {
                RIGHT_CONTEXT_MS
            };
            if !quiet_last && word.end + required_context > audio_end_ms {
                break;
            }
            if !self.recent.iter().all(|snapshot| {
                snapshot.get(index).is_some_and(|previous| {
                    (previous.text == word.text
                        || (quiet_last
                            && !word.anchor_key().is_empty()
                            && previous.anchor_key() == word.anchor_key()))
                        && (previous.start - word.start).abs() <= ALIGN_TOLERANCE_MS
                })
            }) {
                break;
            }
            count += 1;
        }
        count
    }

    fn advance_window(&mut self) -> Result<(), String> {
        if self.audio.len() - self.window_start < WINDOW_TARGET {
            return Ok(());
        }
        let origin_ms = samples_ms(self.window_start);
        let end_ms = samples_ms(self.audio.len());
        let next = self.published.last().and_then(|last| {
            if last.text.chars().any(char::is_numeric) {
                return None;
            }
            self.anchor_words(origin_ms).first().and_then(|first| {
                (first.start > origin_ms + 3000).then(|| {
                    let anchor = (first.start as usize * SAMPLE_RATE) / 1000;
                    anchor.saturating_sub(CONTEXT) / FRAME * FRAME
                })
            })
        });
        if let Some(next) = next.filter(|next| *next >= self.window_start + CONTEXT) {
            self.window_start = next;
            self.recent.clear();
            return Ok(());
        }
        let no_new_words = self.recent.len() == AGREEMENTS && self.recent.iter().all(Vec::is_empty);
        let old_anchor = self
            .published
            .last()
            .is_none_or(|word| word.end + 4000 + ALIGN_TOLERANCE_MS < end_ms);
        if no_new_words && old_anchor {
            self.window_start = self.audio.len().saturating_sub(QUIET_CONTEXT) / FRAME * FRAME;
            self.recent.clear();
            return Ok(());
        }
        if self.audio.len() - self.window_start >= WINDOW_LIMIT {
            return Err(
                "Local dictation could not establish a safe overlap; further insertion stopped"
                    .into(),
            );
        }
        Ok(())
    }
}

fn samples_ms(samples: usize) -> i64 {
    (samples as i64 * 1000) / SAMPLE_RATE as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(words: &[(&str, i64, i64)]) -> Transcript {
        Transcript {
            text: words
                .iter()
                .map(|word| word.0)
                .collect::<Vec<_>>()
                .join(" "),
            words: words
                .iter()
                .map(|(text, start, end)| Word {
                    text: (*text).into(),
                    t0_ms: *start,
                    t1_ms: *end,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn revised_previews_do_not_insert_until_stable_and_final_suffix_is_once() {
        let mut stream = TdtStream::new();
        let mut answers = VecDeque::from([
            transcript(&[("Hello", 100, 400), ("wrong", 700, 900)]),
            transcript(&[("Hello", 100, 400), ("there", 700, 1500)]),
            transcript(&[("Hello", 100, 400), ("there", 700, 2100)]),
            transcript(&[("Hello", 100, 400), ("there", 700, 2700)]),
            transcript(&[
                ("hello,", 100, 400),
                ("there", 700, 900),
                ("friend.", 1000, 1200),
            ]),
        ]);
        let mut decode = |_: &[f32]| Ok(answers.pop_front().unwrap());
        let block = vec![0.25; PREVIEW_INTERVAL];
        assert_eq!(stream.feed(&block, &mut decode).unwrap(), None);
        assert_eq!(stream.feed(&block, &mut decode).unwrap(), None);
        assert_eq!(stream.feed(&block, &mut decode).unwrap(), None);
        assert_eq!(
            stream.feed(&block, &mut decode).unwrap(),
            Some("Hello".into())
        );
        assert_eq!(stream.feed(&block, &mut decode).unwrap(), None);
        assert_eq!(
            stream.finish(&mut decode).unwrap(),
            Some("Hello there friend.".into())
        );
        assert!(answers.is_empty());
    }

    #[test]
    fn quiet_word_can_arrive_before_stop_when_only_punctuation_changes() {
        let mut stream = TdtStream::new();
        let mut answers = VecDeque::from([
            transcript(&[("Hello,", 100, 400)]),
            transcript(&[("Hello.", 100, 400)]),
            transcript(&[("Hello", 100, 400)]),
            transcript(&[("Hello!", 100, 400)]),
        ]);
        let mut decode = |_: &[f32]| Ok(answers.pop_front().unwrap());
        let quiet = vec![0.002; PREVIEW_INTERVAL];
        assert_eq!(stream.feed(&quiet, &mut decode).unwrap(), None);
        assert_eq!(stream.feed(&quiet, &mut decode).unwrap(), None);
        assert_eq!(stream.feed(&quiet, &mut decode).unwrap(), None);
        assert_eq!(
            stream.feed(&quiet, &mut decode).unwrap(),
            Some("Hello".into())
        );
        assert_eq!(stream.finish(&mut decode).unwrap(), Some("Hello!".into()));
    }

    #[test]
    fn steady_noise_after_speech_releases_a_repeated_last_word() {
        let mut stream = TdtStream::new();
        let mut answers = VecDeque::from([
            transcript(&[("Hello,", 100, 400)]),
            transcript(&[("Hello.", 100, 400)]),
            transcript(&[("Hello", 100, 400)]),
        ]);
        let mut decode = |_: &[f32]| Ok(answers.pop_front().unwrap());
        let speech = vec![0.1; PREVIEW_INTERVAL];
        let noise = vec![0.01; PREVIEW_INTERVAL];
        assert_eq!(stream.feed(&speech, &mut decode).unwrap(), None);
        assert_eq!(stream.feed(&noise, &mut decode).unwrap(), None);
        assert_eq!(stream.feed(&noise, &mut decode).unwrap(), None);
        assert_eq!(
            stream.feed(&noise, &mut decode).unwrap(),
            Some("Hello".into())
        );
    }

    #[test]
    fn punctuation_is_appended_before_the_next_word_without_replaying_it() {
        let mut stream = TdtStream::new();
        let mut answers = VecDeque::from([
            transcript(&[("Hello,", 100, 400)]),
            transcript(&[("Hello.", 100, 400)]),
            transcript(&[("Hello", 100, 400)]),
            transcript(&[("Hello,", 100, 400), ("there", 1300, 1700)]),
            transcript(&[("Hello,", 100, 400), ("there", 1300, 1700)]),
            transcript(&[("Hello,", 100, 400), ("there", 1300, 1700)]),
            transcript(&[("Hello,", 100, 400), ("there.", 1300, 1700)]),
        ]);
        let mut decode = |_: &[f32]| Ok(answers.pop_front().unwrap());
        let quiet = vec![0.002; PREVIEW_INTERVAL];
        for _ in 0..3 {
            assert_eq!(stream.feed(&quiet, &mut decode).unwrap(), None);
        }
        assert_eq!(
            stream.feed(&quiet, &mut decode).unwrap(),
            Some("Hello".into())
        );
        for _ in 0..2 {
            assert_eq!(stream.feed(&quiet, &mut decode).unwrap(), None);
        }
        assert_eq!(
            stream.feed(&quiet, &mut decode).unwrap(),
            Some("Hello, there".into())
        );
        assert_eq!(
            stream.finish(&mut decode).unwrap(),
            Some("Hello, there.".into())
        );
    }

    #[test]
    fn repeated_overlap_anchors_keep_new_words_in_order() {
        let mut stream = TdtStream::new();
        stream.published = vec![
            TimedWord {
                text: "go".into(),
                start: 1000,
                end: 1250,
            },
            TimedWord {
                text: "go".into(),
                start: 1400,
                end: 1650,
            },
            TimedWord {
                text: "go".into(),
                start: 1800,
                end: 2050,
            },
        ];
        let current = vec![
            TimedWord {
                text: "go".into(),
                start: 1010,
                end: 1260,
            },
            TimedWord {
                text: "go".into(),
                start: 1410,
                end: 1660,
            },
            TimedWord {
                text: "go".into(),
                start: 1810,
                end: 2060,
            },
            TimedWord {
                text: "go".into(),
                start: 2200,
                end: 2450,
            },
        ];
        assert_eq!(stream.anchor_end(&current, 0), Some(3));
        assert_eq!(stream.anchor_end(&current[3..], 0), None);
    }

    #[test]
    fn moving_a_window_keeps_context_before_the_entire_anchor() {
        let mut stream = TdtStream::new();
        stream.window_start = SAMPLE_RATE * 14;
        stream.audio = vec![0.25; SAMPLE_RATE * 30];
        stream.published = vec![
            TimedWord {
                text: "twenty-five".into(),
                start: 18_840,
                end: 19_560,
            },
            TimedWord {
                text: "to".into(),
                start: 19_560,
                end: 19_720,
            },
            TimedWord {
                text: "thirty".into(),
                start: 19_720,
                end: 20_280,
            },
        ];
        stream.advance_window().unwrap();
        assert!(stream.window_start > SAMPLE_RATE * 14);
        assert!(samples_ms(stream.window_start) <= 18_840 - samples_ms(CONTEXT));
        assert_eq!(
            stream.anchor_words(samples_ms(stream.window_start)).len(),
            3
        );
    }

    #[test]
    fn changed_committed_words_fail_closed_on_finish() {
        let mut stream = TdtStream::new();
        stream.audio = vec![0.25; SAMPLE_RATE * 3];
        stream.published.push(TimedWord {
            text: "original".into(),
            start: 100,
            end: 500,
        });
        stream.text = "original".into();
        let mut decode = |_: &[f32]| {
            Ok(transcript(&[
                ("replacement", 100, 500),
                ("tail", 900, 1200),
            ]))
        };
        assert!(stream.finish(&mut decode).is_err());
        assert_eq!(stream.text, "original");
    }

    #[test]
    fn long_empty_windows_retain_context_and_are_bounded() {
        let mut stream = TdtStream::new();
        stream.audio = vec![0.0; WINDOW_TARGET];
        stream.recent = VecDeque::from([Vec::new(), Vec::new(), Vec::new()]);
        stream.advance_window().unwrap();
        assert_eq!(stream.audio.len() - stream.window_start, QUIET_CONTEXT);
        stream.audio.resize(MAX_SAMPLES, 0.0);
        let mut decode = |_: &[f32]| Ok(transcript(&[]));
        assert!(stream.feed(&[0.0], &mut decode).is_err());
    }

    #[test]
    fn quiet_rebase_waits_until_the_previous_anchor_is_unambiguously_outside() {
        let mut stream = TdtStream::new();
        stream.audio = vec![0.0; WINDOW_TARGET];
        stream.published.push(TimedWord {
            text: "42".into(),
            start: 7000,
            end: 7500,
        });
        stream.recent = VecDeque::from([Vec::new(), Vec::new(), Vec::new()]);
        stream.advance_window().unwrap();
        assert_eq!(stream.window_start, 0);
        stream.audio.resize(WINDOW_TARGET + SAMPLE_RATE, 0.0);
        stream.advance_window().unwrap();
        assert!(samples_ms(stream.window_start) > 7500 + ALIGN_TOLERANCE_MS);
    }
}
